// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure decisions behind "Open With".
//!
//! GIO supplies applications, icons and content types; this module decides
//! which rows the submenu shows, which section an application belongs to in
//! the chooser, and whether a search matches. Keeping these rules here means
//! they can be tested against plain data, without installed desktop entries.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::search::SearchQuery;

/// At most this many applications follow the default in the submenu.
pub const SUBMENU_OTHERS: usize = 8;

/// One application as the pure decisions need it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// The desktop id, for example `org.kde.kate.desktop`.
    pub id: String,
    /// The display name GIO reports.
    pub name: String,
    /// GIO's `should_show`: false hides the application from every list.
    pub should_show: bool,
    /// The content types this application handles.
    pub handles: Vec<String>,
    /// The desktop entry's `Keywords`, already split on `;`.
    pub keywords: Vec<String>,
}

impl AppEntry {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            should_show: true,
            handles: Vec::new(),
            keywords: Vec::new(),
        }
    }

    pub fn handles(mut self, content_type: impl Into<String>) -> Self {
        self.handles.push(content_type.into());
        self
    }

    pub fn hidden(mut self) -> Self {
        self.should_show = false;
        self
    }

    pub fn keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords.push(keyword.into());
        self
    }
}

/// Whether `app` handles every selected type.
pub fn handles_all(app: &AppEntry, types: &[String]) -> bool {
    !types.is_empty()
        && types
            .iter()
            .all(|content_type| app.handles.iter().any(|handled| handled == content_type))
}

/// The submenu: the default first, then GIO's recommended order, without
/// duplicates or hidden applications, capped at [`SUBMENU_OTHERS`] after the
/// default.
///
/// `recommended` is the concatenation of GIO's recommended lists for the
/// selected types, in GIO's order. For several types the caller passes every
/// type's list; an application must handle all of them to survive.
pub fn submenu_apps(
    default: Option<&AppEntry>,
    recommended: &[AppEntry],
    types: &[String],
) -> Vec<AppEntry> {
    let mut seen = Vec::new();
    let mut apps = Vec::new();

    if let Some(app) = default
        && app.should_show
        && handles_all(app, types)
    {
        seen.push(app.id.clone());
        apps.push(app.clone());
    }

    let mut others = 0;
    for app in recommended {
        if others >= SUBMENU_OTHERS {
            break;
        }
        if !app.should_show || !handles_all(app, types) || seen.iter().any(|id| id == &app.id) {
            continue;
        }
        seen.push(app.id.clone());
        apps.push(app.clone());
        others += 1;
    }

    apps
}

/// The chooser's two sections. Both are sorted by display name, ignoring case
/// and accents; the id breaks ties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChooserSections {
    /// Applications that handle every selected type.
    pub suggested: Vec<AppEntry>,
    /// Every other visible application.
    pub all: Vec<AppEntry>,
}

/// Split visible applications into `Suggested` and `All Applications`.
///
/// Duplicates by desktop id are dropped. An application without a desktop id
/// cannot be launched by `file.open-with`, so it is skipped.
pub fn chooser_sections(apps: &[AppEntry], types: &[String]) -> ChooserSections {
    let mut seen = Vec::new();
    let mut suggested = Vec::new();
    let mut all = Vec::new();

    for app in apps {
        if !app.should_show || app.id.is_empty() {
            continue;
        }
        if seen.iter().any(|id| id == &app.id) {
            continue;
        }
        seen.push(app.id.clone());
        if handles_all(app, types) {
            suggested.push(app.clone());
        } else {
            all.push(app.clone());
        }
    }

    sort_by_name(&mut suggested);
    sort_by_name(&mut all);
    ChooserSections { suggested, all }
}

fn sort_by_name(apps: &mut [AppEntry]) {
    apps.sort_by(|left, right| {
        fold(&left.name)
            .cmp(&fold(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// Whether a search query matches an application's display name, desktop id or
/// one of its keywords, ignoring case and accents.
///
/// Matching uses P2o's matcher (`crate::search::SearchQuery`, from
/// `fm-core/src/search/matcher.rs`): exact, prefix and substring ranks, then
/// ordered-character subsequences and bounded typos for queries of three or
/// more scalars. The matcher itself only lowercases, so this module folds the
/// text first and the matcher sees unaccented characters.
pub fn search_matches(query: &str, app: &AppEntry) -> bool {
    if query.trim().is_empty() {
        return true;
    }

    let query = SearchQuery::new(fold(query));
    let name = fold(&app.name);
    if query.matches_name(OsStr::new(&name)) {
        return true;
    }

    let id = fold(&app.id);
    let stem = id.strip_suffix(".desktop").unwrap_or(&id);
    if query.matches_name(OsStr::new(&id)) || query.matches_name(OsStr::new(stem)) {
        return true;
    }

    app.keywords
        .iter()
        .any(|keyword| query.matches_name(OsStr::new(&fold(keyword))))
}

/// Lowercase and remove common Latin diacritics. The search compares the
/// folded query with folded names, ids and keywords, so `cafe` finds `Café`.
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|ch| !('\u{0300}'..='\u{036f}').contains(ch))
        .map(fold_char)
        .collect()
}

fn fold_char(ch: char) -> char {
    match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'æ' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' | 'đ' | 'ð' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ğ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => 'i',
        'ł' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' | 'œ' => 'o',
        'ř' => 'r',
        'ś' | 'š' | 'ş' | 'ș' | 'ß' => 's',
        'ť' | 'ț' | 'þ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        _ => ch,
    }
}

/// Why `file.open-with` refuses an application or a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenWithProblem {
    UnknownApp(String),
    HiddenApp(String),
    Folder(PathBuf),
}

impl OpenWithProblem {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownApp(id) => format!("No installed application has the desktop id {id}."),
            Self::HiddenApp(name) => format!("{name} is hidden and cannot open files."),
            Self::Folder(_) => "Open With is for files; folders open in Gimbal.".to_string(),
        }
    }
}

/// Refuse an unknown desktop id or an application GIO hides.
pub fn check_app(id: &str, app: Option<&AppEntry>) -> Result<(), OpenWithProblem> {
    let Some(app) = app else {
        return Err(OpenWithProblem::UnknownApp(id.to_string()));
    };
    if !app.should_show {
        return Err(OpenWithProblem::HiddenApp(app.name.clone()));
    }
    Ok(())
}

/// The first folder in a selection, if any. Folders open in Gimbal.
pub fn folder_in_selection(paths: &[PathBuf], is_dir: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    paths.iter().find(|path| is_dir(path)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn app(id: &str) -> AppEntry {
        AppEntry::new(id, id.trim_end_matches(".desktop"))
    }

    #[test]
    fn the_default_is_first_then_recommended_in_gio_order() {
        let default = app("default.desktop").handles("text/plain");
        let first = app("first.desktop").handles("text/plain");
        let second = app("second.desktop").handles("text/plain");
        let apps = submenu_apps(
            Some(&default),
            &[first.clone(), second.clone()],
            &types(&["text/plain"]),
        );
        assert_eq!(
            apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            ["default.desktop", "first.desktop", "second.desktop"]
        );
    }

    #[test]
    fn duplicates_by_desktop_id_are_dropped() {
        let default = app("kate.desktop").handles("text/plain");
        let recommended = [
            app("kate.desktop").handles("text/plain"),
            app("other.desktop").handles("text/plain"),
            app("other.desktop").handles("text/plain"),
        ];
        let apps = submenu_apps(Some(&default), &recommended, &types(&["text/plain"]));
        assert_eq!(
            apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            ["kate.desktop", "other.desktop"]
        );
    }

    #[test]
    fn hidden_applications_are_dropped_even_as_the_default() {
        let default = app("hidden-default.desktop").handles("text/plain").hidden();
        let visible = app("visible.desktop").handles("text/plain");
        let hidden = app("hidden.desktop").handles("text/plain").hidden();
        let apps = submenu_apps(
            Some(&default),
            &[hidden, visible.clone()],
            &types(&["text/plain"]),
        );
        assert_eq!(
            apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            ["visible.desktop"]
        );
    }

    #[test]
    fn the_submenu_caps_the_other_applications_at_eight() {
        let default = app("default.desktop").handles("text/plain");
        let recommended: Vec<AppEntry> = (0..10)
            .map(|index| app(&format!("app-{index}.desktop")).handles("text/plain"))
            .collect();
        let apps = submenu_apps(Some(&default), &recommended, &types(&["text/plain"]));
        assert_eq!(apps.len(), SUBMENU_OTHERS + 1);
        assert_eq!(apps[0].id, "default.desktop");
        assert_eq!(apps[8].id, "app-7.desktop");

        // With no default there are still at most eight recommended rows.
        let apps = submenu_apps(None, &recommended, &types(&["text/plain"]));
        assert_eq!(apps.len(), SUBMENU_OTHERS);
        assert_eq!(apps[7].id, "app-7.desktop");
    }

    #[test]
    fn two_text_types_share_the_editor_that_handles_both() {
        let editor = app("editor.desktop")
            .handles("text/plain")
            .handles("text/markdown");
        let plain_only = app("plain.desktop").handles("text/plain");
        let markdown_only = app("markdown.desktop").handles("text/markdown");
        let apps = submenu_apps(
            None,
            &[plain_only, editor.clone(), markdown_only],
            &types(&["text/plain", "text/markdown"]),
        );
        assert_eq!(
            apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            ["editor.desktop"]
        );
        assert!(handles_all(
            &editor,
            &types(&["text/plain", "text/markdown"])
        ));
    }

    #[test]
    fn an_image_and_a_text_file_with_no_common_application_have_no_rows() {
        let text = app("editor.desktop").handles("text/plain");
        let image = app("photos.desktop").handles("image/png");
        let apps = submenu_apps(None, &[text, image], &types(&["text/plain", "image/png"]));
        assert!(apps.is_empty());
    }

    #[test]
    fn one_type_keeps_every_visible_application_that_handles_it() {
        let first = app("first.desktop").handles("text/plain");
        let second = app("second.desktop").handles("text/plain");
        let other = app("other.desktop").handles("image/png");
        let apps = submenu_apps(None, &[first, second, other], &types(&["text/plain"]));
        assert_eq!(
            apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(),
            ["first.desktop", "second.desktop"]
        );
    }

    #[test]
    fn the_chooser_puts_suggested_applications_first_and_sorts_both_sections() {
        let zed = app("zed.desktop").handles("text/plain");
        let kate = app("kate.desktop").handles("text/plain");
        let photos = app("photos.desktop").handles("image/png");
        let hidden = app("hidden.desktop").handles("text/plain").hidden();
        let sections = chooser_sections(&[zed, kate, photos, hidden], &types(&["text/plain"]));
        assert_eq!(
            sections
                .suggested
                .iter()
                .map(|app| app.id.as_str())
                .collect::<Vec<_>>(),
            ["kate.desktop", "zed.desktop"]
        );
        assert_eq!(
            sections
                .all
                .iter()
                .map(|app| app.id.as_str())
                .collect::<Vec<_>>(),
            ["photos.desktop"]
        );
    }

    #[test]
    fn search_matches_the_display_name_desktop_id_and_keywords_ignoring_case() {
        let kate = AppEntry::new("org.kde.kate.desktop", "Kate")
            .handles("text/plain")
            .keyword("Text Editor")
            .keyword("Programming");

        assert!(search_matches("kate", &kate));
        assert!(search_matches("KATE", &kate));
        assert!(search_matches("org.kde", &kate));
        assert!(search_matches("kde.kate", &kate));
        assert!(search_matches("editor", &kate));
        assert!(search_matches("program", &kate));
        assert!(!search_matches("browser", &kate));
    }

    #[test]
    fn search_ignores_accents_in_names_and_queries() {
        let cafe = AppEntry::new("org.example.cafe.desktop", "Café");
        assert!(search_matches("cafe", &cafe));
        assert!(search_matches("café", &cafe));
        assert!(search_matches("CAFÉ", &cafe));

        let plain = AppEntry::new("org.example.other.desktop", "Cafe");
        assert!(search_matches("café", &plain));

        let decomposed = AppEntry::new("org.example.decomposed.desktop", "Cafe\u{301}");
        assert!(search_matches("cafe", &decomposed));
    }

    #[test]
    fn an_empty_search_matches_everything() {
        let app = AppEntry::new("org.example.any.desktop", "Anything");
        assert!(search_matches("", &app));
        assert!(search_matches("   ", &app));
    }

    #[test]
    fn unknown_and_hidden_applications_are_refused() {
        assert_eq!(
            check_app("missing.desktop", None),
            Err(OpenWithProblem::UnknownApp("missing.desktop".to_string()))
        );

        let hidden = AppEntry::new("hidden.desktop", "Hidden").hidden();
        assert_eq!(
            check_app("hidden.desktop", Some(&hidden)),
            Err(OpenWithProblem::HiddenApp("Hidden".to_string()))
        );

        let visible = AppEntry::new("visible.desktop", "Visible");
        assert_eq!(check_app("visible.desktop", Some(&visible)), Ok(()));
    }

    #[test]
    fn a_folder_in_the_selection_is_refused() {
        let paths = [PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/folder")];
        assert_eq!(
            folder_in_selection(&paths, |path| path.ends_with("folder")),
            Some(PathBuf::from("/tmp/folder"))
        );
        assert_eq!(folder_in_selection(&paths, |_| false), None);
    }
}
