mod platform;

use std::collections::BTreeSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LUMA_DARK: f64 = 65.0;
const MIN_LUMA_LIGHT: f64 = 120.0;
const RECORD_SIZE: usize = 48;
const HEADER_SIZE: usize = 64;
const MAX_INDEXED_ITEMS: usize = u16::MAX as usize + 1;
const CACHE_MAGIC: &[u8; 8] = b"&gTYRND3";
const LAST_LIGHT_OFFSET: u64 = 32;
const LAST_DARK_OFFSET: u64 = 34;
const LAST_FONT_OFFSET: u64 = 36;
const LAST_VALID_OFFSET: u64 = 38;

const HELP: &str = "ghostty-rand - open Ghostty with random themes\n\n\
Usage:\n\
  ghostty-rand [--help | --version] [-- GHOSTTY_ARGS...]\n\
  ghostty-rand --generate [--dark-threshold VALUE] [--light-threshold VALUE]\n\
                 [--font-family FAMILY ...]\n\n\
Options:\n\
  --generate             Cache available light and dark themes\n\
  --dark-threshold VALUE Classify themes below VALUE as dark (default: 65)\n\
  --light-threshold VALUE Classify themes above VALUE as light (default: 120)\n\
  --font-family FAMILY   Add a font family to the random selection (repeatable)\n\
  -- GHOSTTY_ARGS        Pass all following arguments to Ghostty unchanged\n\
  -V, --version          Show the package version\n\
  -h, --help              Show this help\n\n\
Theme discovery honors GHOSTTY_RESOURCES_DIR and common Ghostty install paths.\n\
The cache path can be set with GHOSTTY_RAND_CACHE_BIN.\n\
Otherwise it is stored in ~/.ghostty-rand-cache.bin.\n";

#[derive(Default)]
struct Options {
    generate: bool,
    font_families: Vec<String>,
    dark_threshold: Option<f64>,
    light_threshold: Option<f64>,
    ghostty_args: Vec<OsString>,
    show_help: bool,
    show_version: bool,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("ghostty-rand: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options(env::args_os().skip(1))?;
    if options.show_help {
        print!("{HELP}");
        return Ok(());
    }
    if options.show_version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let dark_threshold = options.dark_threshold.unwrap_or(MAX_LUMA_DARK);
    let light_threshold = options.light_threshold.unwrap_or(MIN_LUMA_LIGHT);
    if dark_threshold >= light_threshold {
        return Err("--dark-threshold must be lower than --light-threshold".to_owned());
    }
    if options.generate && !options.ghostty_args.is_empty() {
        return Err("Ghostty arguments cannot be used with --generate".to_owned());
    }

    let cache_path = cache_path()?;
    if options.generate {
        let counts = generate_cache(
            &cache_path,
            &options.font_families,
            dark_threshold,
            light_threshold,
        )
        .map_err(|error| format!("could not generate cache: {error}"))?;
        println!(
            "Cached {} light themes, {} dark themes, and {} font families.",
            counts.lights, counts.darks, counts.fonts
        );
        println!(
            "Handled exceptions: {} unusable theme files, {} themes between thresholds, {} theme names over 48 bytes, {} non-UTF-8 theme names, {} font names over 48 bytes, {} empty font names.",
            counts.unusable_theme_files,
            counts.between_thresholds,
            counts.long_theme_names,
            counts.non_utf8_theme_names,
            counts.long_font_names,
            counts.empty_font_names,
        );
        if counts.truncated_lights + counts.truncated_darks + counts.truncated_fonts > 0 {
            println!(
                "Truncated to at most {MAX_INDEXED_ITEMS} entries per category: {} light, {} dark, {} font.",
                counts.truncated_lights, counts.truncated_darks, counts.truncated_fonts
            );
        }
        if counts.used_builtin_light || counts.used_builtin_dark {
            println!(
                "Built-in fallback used: light={}, dark={}",
                counts.used_builtin_light, counts.used_builtin_dark
            );
        }
        return Ok(());
    }

    let cache = match read_cache(&cache_path) {
        Ok(cache) => cache,
        Err(_) => {
            generate_cache(&cache_path, &[], MAX_LUMA_DARK, MIN_LUMA_LIGHT)
                .map_err(|error| format!("could not generate theme cache: {error}"))?;
            read_cache(&cache_path)
                .map_err(|error| format!("could not read theme cache: {error}"))?
        }
    };

    let light = cache.light.unwrap_or_else(|| "Builtin Light".to_owned());
    let dark = cache.dark.unwrap_or_else(|| "Builtin Dark".to_owned());
    let font = cache.font;
    match font.as_deref() {
        Some(font) => println!("Light: {light} | Dark: {dark} | Font: {font}"),
        None => println!("Light: {light} | Dark: {dark} | Font: none"),
    }

    let cwd =
        env::current_dir().map_err(|error| format!("could not read current directory: {error}"))?;
    platform::launch(&light, &dark, font.as_ref(), &cwd, &options.ghostty_args)
        .map_err(|error| format!("could not start Ghostty: {error}"))
}

fn parse_options(args: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.peekable();
    let mut forwarding = false;
    while let Some(raw_arg) = args.next() {
        if forwarding {
            options.ghostty_args.push(raw_arg);
            continue;
        }
        if raw_arg.as_os_str() == OsStr::new("--") {
            forwarding = true;
            continue;
        }
        let arg = raw_arg
            .to_str()
            .ok_or_else(|| "options before -- must be UTF-8".to_owned())?;
        match arg {
            "-h" | "--help" => options.show_help = true,
            "-V" | "--version" => options.show_version = true,
            "--generate" => options.generate = true,
            "--dark-threshold" => {
                options.dark_threshold = Some(parse_threshold(
                    "--dark-threshold",
                    Some(next_utf8(&mut args, "--dark-threshold")?),
                )?);
            }
            "--light-threshold" => {
                options.light_threshold = Some(parse_threshold(
                    "--light-threshold",
                    Some(next_utf8(&mut args, "--light-threshold")?),
                )?);
            }
            "--font-family" => {
                let family = next_utf8(&mut args, "--font-family")?;
                if family.starts_with('-') {
                    return Err("--font-family requires a value".to_owned());
                }
                options.font_families.push(family);
            }
            _ if arg.starts_with("--font-family=") => {
                let family = arg["--font-family=".len()..].to_owned();
                if family.is_empty() {
                    return Err("--font-family requires a value".to_owned());
                }
                options.font_families.push(family);
            }
            _ if arg.starts_with("--dark-threshold=") => {
                options.dark_threshold = Some(parse_threshold(
                    "--dark-threshold",
                    Some(arg["--dark-threshold=".len()..].to_owned()),
                )?);
            }
            _ if arg.starts_with("--light-threshold=") => {
                options.light_threshold = Some(parse_threshold(
                    "--light-threshold",
                    Some(arg["--light-threshold=".len()..].to_owned()),
                )?);
            }
            _ => return Err(format!("unknown option: {arg}\n\n{HELP}")),
        }
    }

    if !options.generate
        && (!options.font_families.is_empty()
            || options.dark_threshold.is_some()
            || options.light_threshold.is_some())
    {
        return Err("cache options can only be used with --generate".to_owned());
    }
    if let (Some(dark), Some(light)) = (options.dark_threshold, options.light_threshold) {
        if dark >= light {
            return Err("--dark-threshold must be lower than --light-threshold".to_owned());
        }
    }
    Ok(options)
}

fn next_utf8(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))?
        .into_string()
        .map_err(|_| format!("{flag} value must be UTF-8"))
}

fn parse_threshold(flag: &str, value: Option<String>) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("{flag} requires a value"))?;
    let threshold = value
        .parse::<f64>()
        .map_err(|_| format!("{flag} must be a number between 0 and 255"))?;
    if !threshold.is_finite() || !(0.0..=255.0).contains(&threshold) {
        return Err(format!("{flag} must be a number between 0 and 255"));
    }
    Ok(threshold)
}

struct CacheCounts {
    lights: usize,
    darks: usize,
    fonts: usize,
    unusable_theme_files: usize,
    between_thresholds: usize,
    long_theme_names: usize,
    non_utf8_theme_names: usize,
    long_font_names: usize,
    empty_font_names: usize,
    truncated_lights: usize,
    truncated_darks: usize,
    truncated_fonts: usize,
    used_builtin_light: bool,
    used_builtin_dark: bool,
}

fn cache_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("GHOSTTY_RAND_CACHE_BIN") {
        if path.is_empty() {
            return Err("GHOSTTY_RAND_CACHE_BIN must not be empty".to_owned());
        }
        return Ok(PathBuf::from(path));
    }

    let home = env::var_os("HOME")
        .ok_or_else(|| "could not determine the home directory (HOME is unset)".to_owned())?;
    Ok(PathBuf::from(home).join(".ghostty-rand-cache.bin"))
}

fn generate_cache(
    path: &Path,
    font_families: &[String],
    dark_threshold: f64,
    light_threshold: f64,
) -> io::Result<CacheCounts> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut lights = BTreeSet::new();
    let mut darks = BTreeSet::new();
    let mut unusable_theme_files = 0;
    let mut between_thresholds = 0;
    let mut long_theme_names = 0;
    let mut non_utf8_theme_names = 0;
    let mut visited_theme_dirs = BTreeSet::new();
    for theme_dir in platform::theme_dirs() {
        let canonical_dir = theme_dir
            .canonicalize()
            .unwrap_or_else(|_| theme_dir.clone());
        if !visited_theme_dirs.insert(canonical_dir) {
            continue;
        }
        let Ok(entries) = fs::read_dir(theme_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name() else {
                continue;
            };
            let Some(name) = file_name.to_str() else {
                non_utf8_theme_names += 1;
                continue;
            };
            let Some(luma) = theme_luma(&path) else {
                unusable_theme_files += 1;
                continue;
            };
            if luma >= dark_threshold && luma <= light_threshold {
                between_thresholds += 1;
                continue;
            }
            if name.len() > RECORD_SIZE {
                long_theme_names += 1;
                continue;
            }
            if luma < dark_threshold {
                darks.insert(name.to_owned());
            } else {
                lights.insert(name.to_owned());
            }
        }
    }

    let used_builtin_light = lights.is_empty();
    let used_builtin_dark = darks.is_empty();
    if lights.is_empty() {
        lights.insert("Builtin Light".to_owned());
    }
    if darks.is_empty() {
        darks.insert("Builtin Dark".to_owned());
    }

    let mut fonts = BTreeSet::new();
    let mut long_font_names = 0;
    let mut empty_font_names = 0;
    for family in font_families {
        if family.is_empty() {
            empty_font_names += 1;
        } else if family.len() > RECORD_SIZE {
            long_font_names += 1;
        } else {
            fonts.insert(family.clone());
        }
    }

    let truncated_lights = truncate_set(&mut lights);
    let truncated_darks = truncate_set(&mut darks);
    let truncated_fonts = truncate_set(&mut fonts);

    write_cache(path, &lights, &darks, &fonts)?;

    Ok(CacheCounts {
        lights: lights.len(),
        darks: darks.len(),
        fonts: fonts.len(),
        unusable_theme_files,
        between_thresholds,
        long_theme_names,
        non_utf8_theme_names,
        long_font_names,
        empty_font_names,
        truncated_lights,
        truncated_darks,
        truncated_fonts,
        used_builtin_light,
        used_builtin_dark,
    })
}

fn truncate_set(entries: &mut BTreeSet<String>) -> usize {
    let excess = entries.len().saturating_sub(MAX_INDEXED_ITEMS);
    for _ in 0..excess {
        if let Some(last) = entries.iter().next_back().cloned() {
            entries.remove(&last);
        }
    }
    excess
}

fn theme_luma(path: &Path) -> Option<f64> {
    let contents = fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "background" {
            continue;
        }

        // Match the original awk behavior: inspect the first value token, remove
        // punctuation such as '#', and accept only six hexadecimal digits.
        let token = value.split_whitespace().next().unwrap_or_default();
        let hex: String = token.chars().filter(char::is_ascii_hexdigit).collect();
        if hex.len() != 6 {
            continue;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f64;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f64;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f64;
        return Some(0.299 * r + 0.587 * g + 0.114 * b);
    }
    None
}

fn write_cache(
    path: &Path,
    lights: &BTreeSet<String>,
    darks: &BTreeSet<String>,
    fonts: &BTreeSet<String>,
) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    {
        let mut output = fs::File::create(&temporary)?;

        let mut header = [0_u8; HEADER_SIZE];
        header[..CACHE_MAGIC.len()].copy_from_slice(CACHE_MAGIC);
        header[8..16].copy_from_slice(&(lights.len() as u64).to_le_bytes());
        header[16..24].copy_from_slice(&(darks.len() as u64).to_le_bytes());
        header[24..32].copy_from_slice(&(fonts.len() as u64).to_le_bytes());
        output.write_all(&header)?;

        for entries in [lights, darks, fonts] {
            for entry in entries {
                if !fits_record(entry) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "cache entry does not fit a 48-byte record",
                    ));
                }
                let mut record = [0_u8; RECORD_SIZE];
                record[..entry.len()].copy_from_slice(entry.as_bytes());
                output.write_all(&record)?;
            }
        }
        output.sync_all()?;
    }
    replace_file(&temporary, path)
}

fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary, path)
}

fn fits_record(name: &str) -> bool {
    !name.is_empty() && name.len() <= RECORD_SIZE && !name.as_bytes().contains(&0)
}

struct CacheSelection {
    light: Option<String>,
    dark: Option<String>,
    font: Option<String>,
}

fn read_cache(path: &Path) -> io::Result<CacheSelection> {
    let (mut file, writable) = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => (file, true),
        Err(_) => (fs::File::open(path)?, false),
    };
    let file_len = file.metadata()?.len();

    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != CACHE_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache header is invalid",
        ));
    }

    let mut header = [0_u8; HEADER_SIZE];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;

    let light_count = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let dark_count = u64::from_le_bytes(header[16..24].try_into().unwrap());
    let font_count = u64::from_le_bytes(header[24..32].try_into().unwrap());
    if [light_count, dark_count, font_count]
        .into_iter()
        .any(|count| count > MAX_INDEXED_ITEMS as u64)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache category exceeds the index limit",
        ));
    }
    let record_count = light_count
        .checked_add(dark_count)
        .and_then(|count| count.checked_add(font_count))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cache count overflow"))?;
    let expected_len = (HEADER_SIZE as u64)
        .checked_add(
            record_count
                .checked_mul(RECORD_SIZE as u64)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cache size overflow"))?,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cache size overflow"))?;
    if file_len != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache length does not match its header",
        ));
    }

    let valid = if writable {
        header[LAST_VALID_OFFSET as usize]
    } else {
        eprintln!("ghostty-rand: cache is read-only; repeat avoidance is disabled");
        0
    };
    let last_light = if valid & 0b001 != 0 {
        Some(u16::from_le_bytes(
            header[LAST_LIGHT_OFFSET as usize..LAST_LIGHT_OFFSET as usize + 2]
                .try_into()
                .unwrap(),
        ))
    } else {
        None
    };
    let last_dark = if valid & 0b010 != 0 {
        Some(u16::from_le_bytes(
            header[LAST_DARK_OFFSET as usize..LAST_DARK_OFFSET as usize + 2]
                .try_into()
                .unwrap(),
        ))
    } else {
        None
    };
    let last_font = if valid & 0b100 != 0 {
        Some(u16::from_le_bytes(
            header[LAST_FONT_OFFSET as usize..LAST_FONT_OFFSET as usize + 2]
                .try_into()
                .unwrap(),
        ))
    } else {
        None
    };

    let (light, last_light) = read_non_repeating_entry(&mut file, 0, light_count, last_light)?;
    let (dark, last_dark) =
        read_non_repeating_entry(&mut file, light_count, dark_count, last_dark)?;
    let (font, last_font) =
        read_non_repeating_entry(&mut file, light_count + dark_count, font_count, last_font)?;

    if writable {
        let mut state = [0_u8; 7];
        let mut valid = 0_u8;
        if let Some(index) = last_light {
            state[0..2].copy_from_slice(&index.to_le_bytes());
            valid |= 0b001;
        }
        if let Some(index) = last_dark {
            state[2..4].copy_from_slice(&index.to_le_bytes());
            valid |= 0b010;
        }
        if let Some(index) = last_font {
            state[4..6].copy_from_slice(&index.to_le_bytes());
            valid |= 0b100;
        }
        state[6] = valid;
        if file
            .seek(SeekFrom::Start(LAST_LIGHT_OFFSET))
            .and_then(|_| file.write_all(&state))
            .is_err()
        {
            eprintln!("ghostty-rand: could not update last-picked cache state");
        }
    }

    Ok(CacheSelection { light, dark, font })
}

fn read_non_repeating_entry(
    file: &mut fs::File,
    first_record: u64,
    record_count: u64,
    previous: Option<u16>,
) -> io::Result<(Option<String>, Option<u16>)> {
    if record_count == 0 {
        return Ok((None, None));
    }

    let previous = previous
        .map(u64::from)
        .filter(|index| *index < record_count);
    let random_range = if previous.is_some() && record_count > 1 {
        record_count - 1
    } else {
        record_count
    };
    let mut selected = random_index(random_range);
    if let Some(previous) = previous.filter(|_| record_count > 1) {
        if selected >= previous {
            selected += 1;
        }
    }

    let record_index = first_record + selected;
    let offset = HEADER_SIZE as u64 + record_index * RECORD_SIZE as u64;
    file.seek(SeekFrom::Start(offset))?;
    let mut record = [0_u8; RECORD_SIZE];
    file.read_exact(&mut record)?;
    let end = record
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(RECORD_SIZE);
    let name = std::str::from_utf8(&record[..end])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if name.is_empty() {
        return Ok((None, Some(selected as u16)));
    }
    Ok((Some(name.to_owned()), Some(selected as u16)))
}

fn random_index(count: u64) -> u64 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    hasher.finish() % count
}
