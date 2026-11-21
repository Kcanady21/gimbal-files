// SPDX-License-Identifier: GPL-3.0-or-later
//! The file-type filters an application sends, and what a name matches.
//!
//! A filter is the portal's `(sa(us))`: a name and a list of (kind, pattern)
//! rules, where kind 0 is a shell glob and kind 1 a MIME type
//! (`org.freedesktop.impl.portal.FileChooser.xml`). Nothing here needs a
//! window: the matcher is the part a test can hold, and it follows GTK's own
//! `GtkFileFilter` so an application sees what it sees under the GTK backend.
//!
//! The two entry points differ only in where the file's type comes from:
//! [`Filter::matches`] takes it, [`Filter::matches_name`] guesses it from the
//! name alone (see each).

use gtk::{gio, glib};

use fm_core::folder_size::EntryKind;
use fm_core::search::{SearchCandidate, SearchResultFilter};

/// A glob over the display name, as `gtk_file_filter_add_pattern` does
/// (`gtk/gtkfilefilter.c:672-694`).
pub const RULE_GLOB: u32 = 0;
/// A MIME type, as `gtk_file_filter_add_mime_type` does
/// (`gtk/gtkfilefilter.c:645-669`).
pub const RULE_MIME: u32 = 1;

/// One (kind, pattern) rule. An unknown kind is dropped when the filter is
/// read, exactly as `gtk_file_filter_new_from_gvariant`'s `default: break`
/// does (`gtk/gtkfilefilter.c:1157-1173`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: u32,
    pub pattern: String,
}

/// One named filter, kept as the application sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub rules: Vec<Rule>,
    /// The exact `(sa(us))` variant as it arrived. The result's
    /// `current_filter` is this, so an application gets back the bytes it
    /// chose, and the selected entry is found by comparing whole variants
    /// (`nautilus-portal.c:539`, `g_variant_equal (variant, current_filter)`).
    raw: glib::Variant,
}

impl Filter {
    /// Reads one `(sa(us))`. `None` when the variant is not one at all: a
    /// malformed filter is logged and skipped by the caller, never an error.
    pub fn from_variant(value: &glib::Variant) -> Option<Self> {
        let (name, rules) = value.get::<(String, Vec<(u32, String)>)>()?;
        let rules = rules
            .into_iter()
            .filter(|(kind, _)| *kind == RULE_GLOB || *kind == RULE_MIME)
            .map(|(kind, pattern)| Rule { kind, pattern })
            .collect();
        Some(Self {
            name,
            rules,
            raw: value.clone(),
        })
    }

    /// The `(sa(us))` to answer with: the bytes the application sent.
    pub fn raw(&self) -> &glib::Variant {
        &self.raw
    }

    /// Whether answering a MIME rule would need the file's type. A filter of
    /// globs alone never asks GIO for one.
    pub fn needs_content_type(&self) -> bool {
        self.rules.iter().any(|rule| rule.kind == RULE_MIME)
    }

    /// Whether this filter admits the name, given what GIO knows of its type.
    ///
    /// Any one rule is enough, as in `gtk_file_filter_match`
    /// (`gtk/gtkfilefilter.c:993-1052`). A filter with no rules admits nothing,
    /// which is what an empty rule list means to `GtkFileFilter` too: the loop
    /// over its rules has no other exit.
    pub fn matches(&self, name: &str, content_type: Option<&str>) -> bool {
        self.rules.iter().any(|rule| match rule.kind {
            RULE_GLOB => fnmatch(&rule.pattern, name),
            RULE_MIME => {
                content_type.is_some_and(|kind| gio::content_type_is_a(kind, &rule.pattern))
            }
            _ => false,
        })
    }

    /// Whether this filter admits a name with nothing else known about it — a
    /// search result, where no file may be opened or read during a scan. The
    /// type comes from `g_content_type_guess` on the name alone, so GIO sees no
    /// bytes and falls back to the extension's glob.
    ///
    /// That is honest only as far as the extension is: a file named
    /// `invoice.pdf` that holds a PNG passes an `application/pdf` rule here,
    /// and a `notes.txt` that holds one is dropped. Accepting a result still
    /// checks the path is a regular file — never that its contents agree with
    /// its name (`portal/picker.rs`, `accept_path`).
    pub fn matches_name(&self, name: &str) -> bool {
        self.matches(name, Some(&guessed_type(name)))
    }
}

/// GIO's guess at a name's type, from the name alone: no bytes, so no file is
/// opened or read.
fn guessed_type(name: &str) -> String {
    gio::content_type_guess(Some(name), None).0.to_string()
}

/// What the dialog keeps visible: its folders-only mode, its active filter and
/// its Ctrl+H switch. The same three decide the folder view and the search
/// results, so a result can never be something the grid would not show.
#[derive(Clone, Debug, Default)]
pub struct ViewRules {
    /// The call's `directory`: files never show, and filters are ignored.
    pub folders_only: bool,
    /// The active file-type filter, if the call had one.
    pub filter: Option<Filter>,
    /// This dialog's Ctrl+H, never `config.toml`'s.
    pub show_hidden: bool,
}

impl ViewRules {
    /// Whether a name is visible. Hidden first, so a dotfile folder obeys
    /// Ctrl+H like any other name; then the mode: a folder always shows in a
    /// file picker, so you can still navigate, and in folders-only mode nothing
    /// else does. The filter's rules apply to files alone, which is what makes
    /// the dropdown read the way it looks.
    pub fn keeps(&self, name: &str, is_folder: bool, content_type: Option<&str>) -> bool {
        if !self.show_hidden && name.starts_with('.') {
            return false;
        }
        if self.folders_only {
            return is_folder;
        }
        if is_folder {
            return true;
        }
        self.filter
            .as_ref()
            .is_none_or(|filter| filter.matches(name, content_type))
    }

    /// The grid's view of one listed item. Its type is the
    /// `standard::content-type` the folder listing already asks for
    /// (`folder/sort.rs:103-107`), so no rule costs an extra lookup; its name is
    /// the display name, which is what `GtkFileFilter` matches globs against
    /// (`gtk/gtkfilefilter.c:1013-1022`).
    pub fn keeps_info(&self, info: &gio::FileInfo) -> bool {
        let display = info.display_name();
        let name = if display.is_empty() {
            info.name().to_string_lossy().into_owned()
        } else {
            display.to_string()
        };
        let content_type = info.content_type().map(|kind| kind.to_string());
        self.keeps(
            &name,
            info.file_type() == gio::FileType::Directory,
            content_type.as_deref(),
        )
    }

    /// The type a candidate needs before [`Self::keeps`] can answer, if any.
    fn content_type_for(&self, name: &str, is_folder: bool) -> Option<String> {
        if is_folder || self.folders_only {
            return None;
        }
        self.filter
            .as_ref()
            .filter(|filter| filter.needs_content_type())
            .map(|_| guessed_type(name))
    }
}

/// The dialog's rules in the form the search worker takes: the same `keeps`,
/// run on the worker before a candidate is scored, ranked or counted
/// ([`fm_core::search::SearchResultFilter`]).
#[derive(Debug)]
pub struct SearchFilter(pub ViewRules);

impl SearchResultFilter for SearchFilter {
    fn keeps(&self, candidate: &SearchCandidate<'_>) -> bool {
        let name = candidate.name.to_string_lossy();
        let is_folder = candidate.kind == EntryKind::Directory;
        let content_type = self.0.content_type_for(&name, is_folder);
        self.0.keeps(&name, is_folder, content_type.as_deref())
    }
}

/// The extension a filter's first simple glob names: the first rule of the
/// form `*.ext` with no other wildcard in it. A MIME-only filter, a `*` filter
/// or a glob like `*.tar.?z` names nothing this rule can use.
fn simple_extension(filter: &Filter) -> Option<String> {
    filter.rules.iter().find_map(|rule| {
        if rule.kind != RULE_GLOB {
            return None;
        }
        let extension = rule.pattern.strip_prefix("*.")?;
        if extension.is_empty()
            || extension
                .chars()
                .any(|c| matches!(c, '*' | '?' | '[' | ']' | '\\' | '/'))
        {
            return None;
        }
        Some(extension.to_string())
    })
}

/// Split a display name at its last dot, unless the dot is the first character:
/// `.bashrc` has no extension, while `notes.txt` keeps `notes` and `txt`.
fn split_extension(name: &str) -> Option<(&str, &str)> {
    let dot = name.rfind('.')?;
    if dot == 0 {
        None
    } else {
        Some((&name[..dot], &name[dot + 1..]))
    }
}

/// The name with its extension replaced when the filter dropdown changes, or
/// `None` to leave the name alone.
///
/// The new filter's first simple `*.ext` glob supplies the new extension. It
/// applies only when the current name's extension matches one of the **old**
/// filter's globs, ASCII case-insensitively: a name with no extension, an
/// extension the old filter does not list, a MIME-only filter or a `*` filter
/// on either side changes nothing.
///
/// The rule is GTK's and Nautilus's (`nautilus-file-chooser.c:411-510`): change
/// the suffix with the filter, but never guess one for a name the old filter
/// did not claim.
pub fn follow_extension(name: &str, old: Option<&Filter>, new: Option<&Filter>) -> Option<String> {
    let new_extension = simple_extension(new?)?;
    let old = old?;
    let (stem, _) = split_extension(name)?;
    let lower_name = name.to_ascii_lowercase();
    let matches_old = old.rules.iter().any(|rule| {
        // A bare `*` matches every name but claims no extension, and neither
        // does a glob with no dot in it. Only an extension-bearing glob can
        // hand its suffix to the new filter.
        rule.kind == RULE_GLOB
            && rule.pattern.contains('.')
            && fnmatch(&rule.pattern.to_ascii_lowercase(), &lower_name)
    });
    if !matches_old {
        return None;
    }
    Some(format!("{stem}.{new_extension}"))
}

/// Shell glob matching with `fnmatch` rules: `*`, `?`, `[abc]`, `[!abc]`,
/// ranges, and a backslash escape.
///
/// Matched a character at a time, so a non-ASCII name is one name, and
/// case-sensitive: on Linux `*.jpg` does not match `A.JPG`, which GTK documents
/// (`gtk/gtkfilefilter.c:672-690`) and GTK applications rely on by sending
/// case-insensitive suffixes as bracket patterns such as `*.[jJ][pP][gG]`.
///
/// The behaviour is GTK's `_gtk_fnmatch` with the flags `GtkFileFilter` uses —
/// `no_leading_period = FALSE`, `casefold = FALSE`, escapes on
/// (`gtk-4.22.4/gtk/gtkfilefilter.c:1015-1023`) — ported from
/// `gimbal-refs/gtk-4.22.4/gtk/fnmatch.c`, © 1991–1993 Free Software
/// Foundation, Inc., LGPL-2.1-or-later. Only the flag combination that file
/// uses is kept: with `no_leading_period` off, the leading-period rule is dead
/// and dropped. A path separator still fails `?` and a character class, as it
/// does there, though a display name never holds one.
///
/// LGPL-2.1-or-later is compatible with this project's GPL-3.0-or-later: the
/// "or later" clause allows the code to be taken under LGPL-3.0, which the GPL-3
/// links with, and Gimbal ships all of its own source, so the work can be
/// relinked against a modified copy.
pub fn fnmatch(pattern: &str, text: &str) -> bool {
    intern(
        &pattern.chars().collect::<Vec<char>>(),
        &text.chars().collect::<Vec<char>>(),
    )
}

/// The character at `index`, or the end-of-string marker GTK reads there. A
/// name cannot hold a NUL, so the marker cannot be mistaken for one.
fn at(chars: &[char], index: usize) -> char {
    chars.get(index).copied().unwrap_or('\0')
}

/// The inner matcher. `ni` is always the index of the character `nc` holds, and
/// is advanced once per pattern character, as GTK's `n` is.
fn intern(pattern: &[char], text: &[char]) -> bool {
    let mut pi = 0usize;
    let mut ni = 0usize;
    while pi < pattern.len() {
        // The position of the character `nc` holds, as in GTK's `last_n`.
        let mut last_n = ni;
        let c = at(pattern, pi);
        pi += 1;
        let mut nc = at(text, ni);
        match c {
            '?' => {
                if nc == '\0' || nc == '/' {
                    return false;
                }
            }
            '\\' => {
                // No `FNM_NOESCAPE` on Linux: the backslash escapes.
                let escaped = at(pattern, pi);
                pi += 1;
                if nc != escaped {
                    return false;
                }
            }
            '*' => {
                // The run of wildcards that follows this one, scanned with an
                // index of its own: this arm always returns, so `pi` is left
                // where the next pattern character will be read from.
                let mut last_p = pi;
                let mut rest = at(pattern, pi);
                pi += 1;
                while rest == '?' || rest == '*' {
                    if rest == '?' {
                        if nc == '\0' || nc == '/' {
                            return false;
                        }
                        ni += 1;
                        last_n = ni;
                        nc = at(text, ni);
                    }
                    last_p = pi;
                    rest = at(pattern, pi);
                    pi += 1;
                }
                if rest == '\0' {
                    // Nothing but wildcards is left: all of the remainder is
                    // the match, unless a separator sits in it.
                    return !text[last_n.min(text.len())..].contains(&'/');
                }
                if rest == '\\' {
                    // The escape is read but `pi` is not moved on: `last_p`
                    // already points at this backslash, and the recursive call
                    // re-reads it as the literal character.
                    rest = at(pattern, pi);
                }
                // Otherwise the rest of the pattern has to match from some
                // later position in the text.
                let mut probe = last_n;
                while nc != '\0' {
                    if (rest == '[' || nc == rest) && intern(&pattern[last_p..], &text[probe..]) {
                        return true;
                    }
                    probe += 1;
                    nc = at(text, probe);
                }
                return false;
            }
            '[' => {
                if nc == '\0' || nc == '/' {
                    return false;
                }
                let not = matches!(at(pattern, pi), '!' | '^');
                if not {
                    pi += 1;
                }
                // The first character of the class, unescaped. Its escape flag
                // is not kept: the loop below re-reads before it asks, as GTK's
                // does, so the value here would never be read.
                let mut c = at(pattern, pi);
                pi += 1;
                if c == '\\' {
                    c = at(pattern, pi);
                    pi += 1;
                }
                let mut was_escaped;
                let mut matched = false;
                loop {
                    if c == '\0' {
                        // `[` left unterminated: the rule loses.
                        return false;
                    }
                    let cstart = c;
                    let (next, escaped) = unescaped(pattern, &mut pi);
                    c = next;
                    was_escaped = escaped;
                    let mut cend = cstart;
                    if !was_escaped && c == '-' && at(pattern, pi) != ']' {
                        let (end, escaped) = unescaped(pattern, &mut pi);
                        was_escaped = escaped;
                        if end == '\0' {
                            return false;
                        }
                        cend = end;
                        c = at(pattern, pi);
                        pi += 1;
                    }
                    if nc >= cstart && nc <= cend {
                        matched = true;
                        break;
                    }
                    if !was_escaped && c == ']' {
                        break;
                    }
                }
                if matched {
                    // Skip whatever is left of the class that just matched.
                    while was_escaped || c != ']' {
                        if c == '\0' {
                            // `[...` left unterminated: the rule loses.
                            return false;
                        }
                        let (next, escaped) = unescaped(pattern, &mut pi);
                        c = next;
                        was_escaped = escaped;
                    }
                    if not {
                        return false;
                    }
                } else if !not {
                    return false;
                }
            }
            _ => {
                if c != nc {
                    return false;
                }
            }
        }
        ni += 1;
    }
    at(text, ni) == '\0'
}

/// One character, with a backslash taking the next one literally. The second
/// value is whether it was escaped: GTK needs it to tell `\-` from a range.
fn unescaped(pattern: &[char], pi: &mut usize) -> (char, bool) {
    let c = at(pattern, *pi);
    *pi += 1;
    if c == '\\' {
        let escaped = at(pattern, *pi);
        *pi += 1;
        (escaped, true)
    } else {
        (c, false)
    }
}
