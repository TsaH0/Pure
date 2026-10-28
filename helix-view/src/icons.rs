//! File icons for the file explorer and file picker (`editor.icons`): Nerd
//! Font glyphs with a color each, like nvim-web-devicons. The terminal's
//! font must be a Nerd Font for them to show.

use std::path::Path;

use crate::graphics::Color;

/// A glyph and its color.
pub type Icon = (&'static str, Color);

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const DIRECTORY: Icon = ("\u{f07b}", rgb(0x7ebae4));
const FILE: Icon = ("\u{f15b}", rgb(0x6d8086));

/// By file name (checked first).
const NAMES: &[(&str, Icon)] = &[
    ("Cargo.toml", ("\u{e7a8}", rgb(0xdea584))),
    ("Cargo.lock", ("\u{e7a8}", rgb(0xdea584))),
    ("Makefile", ("\u{e779}", rgb(0x6d8086))),
    ("Dockerfile", ("\u{f0868}", rgb(0x458ee6))),
    ("LICENSE", ("\u{e60a}", rgb(0xd0bf41))),
    (".gitignore", ("\u{e702}", rgb(0xf54d27))),
    (".gitmodules", ("\u{e702}", rgb(0xf54d27))),
    (".gitattributes", ("\u{e702}", rgb(0xf54d27))),
    ("flake.nix", ("\u{f313}", rgb(0x7ebae4))),
    ("package.json", ("\u{e71e}", rgb(0xe8274b))),
];

/// By extension (lowercase).
const EXTENSIONS: &[(&str, Icon)] = &[
    ("rs", ("\u{e7a8}", rgb(0xdea584))),
    ("py", ("\u{e606}", rgb(0xffbc03))),
    ("js", ("\u{e74e}", rgb(0xcbcb41))),
    ("mjs", ("\u{e74e}", rgb(0xcbcb41))),
    ("ts", ("\u{e628}", rgb(0x519aba))),
    ("jsx", ("\u{e7ba}", rgb(0x20c2e3))),
    ("tsx", ("\u{e7ba}", rgb(0x1354bf))),
    ("json", ("\u{e60b}", rgb(0xcbcb41))),
    ("toml", ("\u{e6b2}", rgb(0x9c4221))),
    ("yaml", ("\u{e6a8}", rgb(0x6d8086))),
    ("yml", ("\u{e6a8}", rgb(0x6d8086))),
    ("md", ("\u{e73e}", rgb(0xdddddd))),
    ("lua", ("\u{e620}", rgb(0x51a0cf))),
    ("go", ("\u{e627}", rgb(0x00add8))),
    ("c", ("\u{e61e}", rgb(0x599eff))),
    ("h", ("\u{f0fd}", rgb(0xa074c4))),
    ("cpp", ("\u{e61d}", rgb(0xf34b7d))),
    ("cc", ("\u{e61d}", rgb(0xf34b7d))),
    ("hpp", ("\u{f0fd}", rgb(0xa074c4))),
    ("java", ("\u{e738}", rgb(0xcc3e44))),
    ("kt", ("\u{e634}", rgb(0x7f52ff))),
    ("swift", ("\u{e755}", rgb(0xe37933))),
    ("rb", ("\u{e791}", rgb(0x701516))),
    ("php", ("\u{e608}", rgb(0xa074c4))),
    ("hs", ("\u{e777}", rgb(0xa074c4))),
    ("zig", ("\u{e6a9}", rgb(0xf69a1b))),
    ("nix", ("\u{f313}", rgb(0x7ebae4))),
    ("sh", ("\u{e795}", rgb(0x89e051))),
    ("bash", ("\u{e795}", rgb(0x89e051))),
    ("zsh", ("\u{e795}", rgb(0x89e051))),
    ("fish", ("\u{e795}", rgb(0x89e051))),
    ("html", ("\u{e736}", rgb(0xe44d26))),
    ("css", ("\u{e749}", rgb(0x42a5f5))),
    ("scss", ("\u{e603}", rgb(0xf55385))),
    ("vue", ("\u{e6a0}", rgb(0x8dc149))),
    ("svelte", ("\u{e697}", rgb(0xff3e00))),
    ("sql", ("\u{e706}", rgb(0xdad8d8))),
    ("txt", ("\u{f0219}", rgb(0x89e051))),
    ("lock", ("\u{f023}", rgb(0xbbbbbb))),
    ("png", ("\u{e60d}", rgb(0xa074c4))),
    ("jpg", ("\u{e60d}", rgb(0xa074c4))),
    ("jpeg", ("\u{e60d}", rgb(0xa074c4))),
    ("gif", ("\u{e60d}", rgb(0xa074c4))),
    ("svg", ("\u{f0721}", rgb(0xffb13b))),
    ("pdf", ("\u{e67d}", rgb(0xb30b00))),
    ("wasm", ("\u{e6a1}", rgb(0x5c4cdb))),
    ("wit", ("\u{e6a1}", rgb(0x5c4cdb))),
    ("zip", ("\u{f410}", rgb(0xeca517))),
    ("gz", ("\u{f410}", rgb(0xeca517))),
    ("tar", ("\u{f410}", rgb(0xeca517))),
];

/// The icon for `path`.
pub fn for_path(path: &Path, is_dir: bool) -> Icon {
    if is_dir {
        return DIRECTORY;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if let Some((_, icon)) = NAMES.iter().find(|(known, _)| *known == name) {
        return *icon;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    extension
        .and_then(|extension| {
            EXTENSIONS
                .iter()
                .find(|(known, _)| *known == extension)
                .map(|(_, icon)| *icon)
        })
        .unwrap_or(FILE)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn icons_by_name_extension_and_kind() {
        assert_eq!(for_path(Path::new("src"), true), DIRECTORY);
        assert_eq!(for_path(Path::new("a/Cargo.toml"), false).0, "\u{e7a8}");
        assert_eq!(for_path(Path::new("x.toml"), false).0, "\u{e6b2}");
        assert_eq!(for_path(Path::new("main.RS"), false).0, "\u{e7a8}");
        assert_eq!(for_path(Path::new("notes"), false), FILE);
        assert_eq!(for_path(Path::new("weird.xyz"), false), FILE);
    }
}
