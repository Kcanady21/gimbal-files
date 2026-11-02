// SPDX-License-Identifier: GPL-3.0-or-later
//! File-manager clipboard formats.
//!
//! Cut and copy put the same files on the clipboard in the formats Nautilus
//! and Dolphin read:
//!
//! - `gdk::FileList`, which GTK serializes as `text/uri-list`
//!   (`gtk-4.22.4:gdk/gdkcontentserializer.c:1045-1050`);
//! - `x-special/gnome-copied-files`: `cut` or `copy`, then one URI per line
//!   (`nautilus-50.2.2:src/nautilus-clipboard.c:54-66`);
//! - `application/x-kde-cutselection`: `1` for a cut, `0` otherwise
//!   (`kio-6.24.0:src/widgets/paste.cpp:256-263`).
//!
//! The parsing and formatting are pure and tested. The live clipboard is only
//! touched by [`write`], [`read`] and [`clear`], which the app calls from a
//! user action and which no test or agent run calls.

use std::path::PathBuf;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

pub const GNOME: &str = "x-special/gnome-copied-files";
pub const KDE: &str = "application/x-kde-cutselection";
pub const URI_LIST: &str = "text/uri-list";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Cut,
    Copy,
}

impl ClipboardMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ClipboardMode::Cut => "cut",
            ClipboardMode::Copy => "copy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardPayload {
    pub mode: ClipboardMode,
    pub uris: Vec<String>,
}

/// The Nautilus format: `cut`/`copy`, then one URI per line.
pub fn gnome_copied_files(mode: ClipboardMode, uris: &[String]) -> String {
    let mut text = mode.as_str().to_string();
    for uri in uris {
        text.push('\n');
        text.push_str(uri);
    }
    text
}

/// The Dolphin flag: `1` for a cut, `0` for a copy.
pub fn kde_cut_selection(mode: ClipboardMode) -> &'static str {
    match mode {
        ClipboardMode::Cut => "1",
        ClipboardMode::Copy => "0",
    }
}

/// Parse Nautilus's format. Empty lines are invalid, as Nautilus treats them.
pub fn parse_gnome_copied_files(text: &str) -> Result<ClipboardPayload, String> {
    let mut lines = text.split('\n');
    let mode = match lines.next().map(str::trim) {
        Some("cut") => ClipboardMode::Cut,
        Some("copy") => ClipboardMode::Copy,
        _ => return Err("the clipboard must begin with “cut” or “copy”".to_string()),
    };
    let mut uris = Vec::new();
    for line in lines {
        if line.is_empty() {
            return Err("the clipboard must not have empty lines".to_string());
        }
        uris.push(line.trim_end_matches('\r').to_string());
    }
    Ok(ClipboardPayload { mode, uris })
}

/// Parse `text/uri-list`: blank lines and `#` comments are ignored.
pub fn parse_uri_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Dolphin's flag. A missing or malformed value means copy.
pub fn parse_kde_cut_selection(text: &str) -> bool {
    text.trim_start().starts_with('1')
}

/// A Dolphin cut is the KDE flag plus the URI list.
pub fn dolphin_payload(uris: Vec<String>, cut: bool) -> ClipboardPayload {
    ClipboardPayload {
        mode: if cut {
            ClipboardMode::Cut
        } else {
            ClipboardMode::Copy
        },
        uris,
    }
}

/// GNOME's action wins, then Dolphin's, then copy.
pub fn decide_mode(gnome: Option<ClipboardMode>, kde_cut: Option<bool>) -> ClipboardMode {
    gnome.unwrap_or_else(|| {
        if kde_cut == Some(true) {
            ClipboardMode::Cut
        } else {
            ClipboardMode::Copy
        }
    })
}

/// A cut's paste clears the clipboard once the move has finished.
pub fn clear_after_cut(mode: ClipboardMode) -> bool {
    mode == ClipboardMode::Cut
}

/// Whether a clipboard's advertised MIME types look like files.
pub fn formats_have_files<'a>(mime_types: impl IntoIterator<Item = &'a str>) -> bool {
    let mut uri = false;
    let mut file_list = false;
    for mime in mime_types {
        if mime == URI_LIST || mime == GNOME || mime == "application/x-kde4-urilist" {
            uri = true;
        }
        if mime == "application/vnd.portal.files" {
            file_list = true;
        }
    }
    uri || file_list
}

/// Put files on a live clipboard in all three formats.
pub fn write(display: &gdk::Display, mode: ClipboardMode, paths: &[PathBuf]) -> Result<(), String> {
    let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
    let uris: Vec<String> = files.iter().map(|file| file.uri().to_string()).collect();
    let file_list = gdk::FileList::from_array(&files);

    let gnome = gnome_copied_files(mode, &uris);
    let kde = kde_cut_selection(mode);
    let providers = [
        gdk::ContentProvider::for_value(&file_list.to_value()),
        gdk::ContentProvider::for_bytes(GNOME, &glib::Bytes::from_owned(gnome.into_bytes())),
        gdk::ContentProvider::for_bytes(KDE, &glib::Bytes::from_owned(kde.as_bytes().to_vec())),
    ];
    let provider = gdk::ContentProvider::new_union(&providers);
    display
        .clipboard()
        .set_content(Some(&provider))
        .map_err(|error| error.to_string())
}

/// Empty the live clipboard.
pub fn clear(display: &gdk::Display) {
    let _ = display
        .clipboard()
        .set_content(None::<&gdk::ContentProvider>);
}

/// Whether the live clipboard advertises files, for the Paste entry.
pub fn display_has_files(display: &gdk::Display) -> bool {
    let formats = display.clipboard().formats();
    let mimes: Vec<String> = formats
        .mime_types()
        .into_iter()
        .map(|mime| mime.to_string())
        .collect();
    formats_have_files(mimes.iter().map(String::as_str))
}

fn read_text<F>(clipboard: &gdk::Clipboard, mime: &'static str, done: F)
where
    F: FnOnce(Option<String>) + 'static,
{
    let clipboard = clipboard.clone();
    clipboard.read_async(
        &[mime],
        glib::Priority::DEFAULT,
        None::<&gio::Cancellable>,
        move |result| {
            let Ok((stream, _)) = result else {
                done(None);
                return;
            };
            stream.read_bytes_async(
                64 * 1024,
                glib::Priority::DEFAULT,
                None::<&gio::Cancellable>,
                move |result| match result {
                    Ok(bytes) => done(Some(String::from_utf8_lossy(bytes.as_ref()).into_owned())),
                    Err(_) => done(None),
                },
            );
        },
    );
}

/// Read the clipboard's files and cut/copy intent, without blocking the main
/// loop. The callback runs on the main context.
pub fn read<F>(display: &gdk::Display, done: F)
where
    F: FnOnce(Result<ClipboardPayload, String>) + 'static,
{
    let clipboard = display.clipboard();
    let formats = clipboard.formats();
    let has_gnome = formats.contain_mime_type(GNOME);
    let has_kde = formats.contain_mime_type(KDE);

    if has_gnome {
        read_text(&clipboard, GNOME, move |text| match text {
            Some(text) => done(parse_gnome_copied_files(&text)),
            None => done(Err("the clipboard could not be read".to_string())),
        });
        return;
    }

    if has_kde {
        let clip = clipboard.clone();
        read_text(&clipboard, KDE, move |flag| {
            let cut = flag.as_deref().is_some_and(parse_kde_cut_selection);
            let clip = clip.clone();
            read_text(&clip, URI_LIST, move |text| {
                let uris = text.map(|text| parse_uri_list(&text)).unwrap_or_default();
                if uris.is_empty() {
                    done(Err("the clipboard holds no files".to_string()));
                } else {
                    done(Ok(dolphin_payload(uris, cut)));
                }
            });
        });
        return;
    }

    read_text(&clipboard, URI_LIST, move |text| {
        let uris = text.map(|text| parse_uri_list(&text)).unwrap_or_default();
        if uris.is_empty() {
            done(Err("the clipboard holds no files".to_string()));
        } else {
            done(Ok(ClipboardPayload {
                mode: decide_mode(None, None),
                uris,
            }));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "file:///home/kyle/one.txt";
    const TWO: &str = "file:///home/kyle/two.txt";

    #[test]
    fn cut_and_copy_build_the_nautilus_and_dolphin_formats() {
        let uris = vec![ONE.to_string(), TWO.to_string()];
        assert_eq!(
            gnome_copied_files(ClipboardMode::Cut, &uris),
            "cut\nfile:///home/kyle/one.txt\nfile:///home/kyle/two.txt"
        );
        assert_eq!(
            gnome_copied_files(ClipboardMode::Copy, &uris),
            "copy\nfile:///home/kyle/one.txt\nfile:///home/kyle/two.txt"
        );
        assert_eq!(kde_cut_selection(ClipboardMode::Cut), "1");
        assert_eq!(kde_cut_selection(ClipboardMode::Copy), "0");
    }

    #[test]
    fn a_nautilus_copy_parses() {
        let text = format!("copy\n{ONE}\n{TWO}");
        let payload = parse_gnome_copied_files(&text).unwrap();
        assert_eq!(payload.mode, ClipboardMode::Copy);
        assert_eq!(payload.uris, [ONE, TWO]);
    }

    #[test]
    fn a_nautilus_cut_parses() {
        let payload = parse_gnome_copied_files(&format!("cut\n{ONE}")).unwrap();
        assert_eq!(payload.mode, ClipboardMode::Cut);
        assert_eq!(payload.uris, [ONE]);
    }

    #[test]
    fn a_dolphin_cut_parses_from_its_flag_and_uri_list() {
        let text = format!("# a comment\n{ONE}\n\n{TWO}\n");
        let uris = parse_uri_list(&text);
        assert_eq!(uris, [ONE, TWO]);
        assert!(parse_kde_cut_selection("1"));
        assert!(!parse_kde_cut_selection("0"));
        assert!(!parse_kde_cut_selection(""));
        let payload = dolphin_payload(uris, parse_kde_cut_selection("1"));
        assert_eq!(payload.mode, ClipboardMode::Cut);
    }

    #[test]
    fn a_plain_uri_list_is_a_copy() {
        let payload = ClipboardPayload {
            mode: decide_mode(None, None),
            uris: parse_uri_list(&format!("{ONE}\n{TWO}")),
        };
        assert_eq!(payload.mode, ClipboardMode::Copy);
        assert_eq!(payload.uris.len(), 2);
    }

    #[test]
    fn gnome_wins_over_kde_and_a_bad_gnome_string_is_an_error() {
        assert_eq!(
            decide_mode(Some(ClipboardMode::Copy), Some(true)),
            ClipboardMode::Copy
        );
        assert_eq!(
            decide_mode(Some(ClipboardMode::Cut), Some(false)),
            ClipboardMode::Cut
        );
        assert!(parse_gnome_copied_files("paste\nfile:///a").is_err());
        assert!(parse_gnome_copied_files("copy\nfile:///a\n\nfile:///b").is_err());
    }

    #[test]
    fn files_are_recognised_from_the_advertised_formats() {
        assert!(formats_have_files([URI_LIST]));
        assert!(formats_have_files([GNOME]));
        assert!(formats_have_files(["application/x-kde4-urilist"]));
        assert!(formats_have_files(["image/png", URI_LIST]));
        assert!(!formats_have_files(["text/plain"]));
        assert!(!formats_have_files(std::iter::empty()));
    }

    #[test]
    fn a_cut_clears_the_clipboard_after_its_paste() {
        assert!(clear_after_cut(ClipboardMode::Cut));
        assert!(!clear_after_cut(ClipboardMode::Copy));
    }
}
