# ghostty-rand

Open a new Ghostty window with random themes and fonts.

## Installation

```sh
cargo install --git https://github.com/lumiknit/ghostty-rand

# Alternatively, clone the repository and run:
# cargo install --path ./ghostty-rand
```

## Usage

First, generate the cache file for themes and optional fonts.

```sh
ghostty-rand --generate \
  --font-family "FONT1" \
  --font-family "FONT2"
# --font-family is optional. For more options, run ghostty-rand --help.
```

This creates `$HOME/.ghostty-rand-cache.bin`,
which stores the theme and font lists and the last selections.
Ghostty randomly picks light and dark themes, then applies one based on the OS appearance.

After initialization, open a new window with:

```sh
ghostty-rand

# Pass additional Ghostty arguments after '--':
ghostty-rand -- --font-size=14
```

The cache defaults to `$HOME/.ghostty-rand-cache.bin`. Set
`GHOSTTY_RAND_CACHE_BIN` to use a different path. Run `ghostty-rand --version`
to print the installed version.

