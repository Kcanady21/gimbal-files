// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests for the command registry.

use std::collections::HashSet;
use std::path::PathBuf;

use super::*;
use crate::fileops::TrashUri;

fn parse_invocation(tokens: &[&str]) -> Invocation {
    let tokens: Vec<String> = tokens.iter().map(|token| token.to_string()).collect();
    Invocation::parse(&tokens).expect("the test's tokens should parse")
}

fn check(caller: Caller, tokens: &[&str]) -> Result<CheckedCommand, CommandError> {
    let invocation = parse_invocation(tokens);
    Registry::new().validate(caller, &invocation)
}

// -------------------------------------------------------------------------
// The catalogue
// -------------------------------------------------------------------------

#[test]
fn the_v1_catalogue_has_every_reviewed_command_in_order() {
    let expected = [
        "app.quit",
        "app.disk-usage",
        "window.new",
        "window.close",
        "tab.new",
        "tab.close",
        "tab.close-selected",
        "tab.focus",
        "tab.move",
        "tab.open-in-new-window",
        "group.open",
        "group.split",
        "group.close",
        "group.rename",
        "group.add-tab",
        "nav.go",
        "nav.back",
        "nav.forward",
        "nav.up",
        "nav.refresh",
        "nav.focus-path",
        "search.open",
        "selection.all",
        "selection.none",
        "file.open",
        "file.trash",
        "file.copy",
        "file.move",
        "file.rename",
        "file.new-folder",
        "file.undo",
        "file.cut",
        "file.copy-to-clipboard",
        "file.paste",
        "trash.open",
        "trash.restore",
        "trash.empty",
        "trash.delete-permanently",
        "preview.edit",
        "preview.discard",
        "preview.save",
        "preview.save-as",
        "view.set-scale",
        "view.zoom-in",
        "view.zoom-out",
        "view.set-opacity",
        "view.toggle-sidebar",
        "view.toggle-section",
        "view.set-sort",
        "view.clear-folder-sort",
        "agent.set-enabled",
        "pins.add",
        "pins.remove",
        "pins.move",
        "drive.mount",
        "drive.unmount",
        "drive.eject",
    ];
    let actual: Vec<&str> = COMMANDS.iter().map(|spec| spec.name).collect();
    assert_eq!(actual, expected);
}

#[test]
fn names_are_unique() {
    let mut seen = HashSet::new();
    for spec in COMMANDS {
        assert!(seen.insert(spec.name), "duplicate command {}", spec.name);
    }
}

#[test]
fn names_are_area_dot_verb_in_lowercase_kebab_case() {
    for spec in COMMANDS {
        assert!(is_well_formed_name(spec.name), "bad name {}", spec.name);
    }

    assert!(!is_well_formed_name("nav"));
    assert!(!is_well_formed_name("Nav.go"));
    assert!(!is_well_formed_name("nav.Go"));
    assert!(!is_well_formed_name("nav..go"));
    assert!(!is_well_formed_name("nav.-go"));
    assert!(!is_well_formed_name("nav.go-"));
    assert!(!is_well_formed_name("nav.go--now"));
    assert!(!is_well_formed_name("nav.go_now"));
    assert!(is_well_formed_name("tab.close-selected"));
    assert!(is_well_formed_name("preview.save-as"));
}

#[test]
fn summaries_are_one_non_empty_line() {
    for spec in COMMANDS {
        assert!(!spec.summary.is_empty(), "{} has no summary", spec.name);
        assert!(
            !spec.summary.contains('\n'),
            "{} has a multi-line summary",
            spec.name
        );
        assert_eq!(spec.summary.trim(), spec.summary, "{}", spec.name);
    }
}

#[test]
fn exactly_the_two_approved_permanent_commands_delete_permanently() {
    let mut approved = Vec::new();
    for spec in COMMANDS {
        if permanently_deletes(spec.name, spec.effect) {
            approved.push(spec.name);
        }
    }
    assert_eq!(
        approved,
        vec!["trash.empty", "trash.delete-permanently"],
        "only Kyle's two approved permanent-delete commands may exist"
    );

    assert!(permanently_deletes("trash.empty", Effect::Files));
    assert!(permanently_deletes(
        "trash.delete-permanently",
        Effect::Files
    ));
    assert!(permanently_deletes("file.delete", Effect::Files));
    assert!(permanently_deletes("file.remove", Effect::Files));
    assert!(!permanently_deletes("file.trash", Effect::Files));
    assert!(!permanently_deletes("pins.remove", Effect::Settings));
    // A differently named Trash deletion is still detected and would be
    // refused by the catalogue rule, not waved through as ordinary trashing.
    assert!(permanently_deletes("trash.delete", Effect::Files));
    assert!(!permanently_deletes(
        "trash.delete-permanently",
        Effect::View
    ));

    // Both approved commands are UI-only for every other caller, including a
    // dry run.
    for command in ["trash.empty", "trash.delete-permanently"] {
        assert!(caller_allowed(command, Effect::Files, Caller::Ui));
        for caller in [Caller::Hotkey, Caller::Cli, Caller::Agent] {
            assert!(!caller_allowed(command, Effect::Files, caller));
        }
    }
    for caller in [Caller::Hotkey, Caller::Cli, Caller::Agent] {
        let error = check(caller, &["trash.empty"]).expect_err("only the UI may empty");
        assert_eq!(error.command(), Some("trash.empty"));
        assert_eq!(error.argument(), None);
        assert!(matches!(
            error.kind(),
            CommandErrorKind::ForbiddenCaller { caller: actual } if *actual == caller
        ));
        assert!(error.to_string().contains("may not run this command"));

        let error = check(
            caller,
            &["trash.delete-permanently", "items=trash:///a.txt"],
        )
        .expect_err("only the UI may delete selected items");
        assert_eq!(error.command(), Some("trash.delete-permanently"));
        assert_eq!(error.argument(), None);
        assert!(matches!(
            error.kind(),
            CommandErrorKind::ForbiddenCaller { caller: actual } if *actual == caller
        ));
        let dry = check(
            caller,
            &[
                "trash.delete-permanently",
                "items=trash:///a.txt",
                "dry-run=true",
            ],
        )
        .expect_err("even a dry run is refused");
        assert!(matches!(
            dry.kind(),
            CommandErrorKind::ForbiddenCaller { .. }
        ));
    }
    for caller in [Caller::Ui, Caller::Hotkey, Caller::Cli, Caller::Agent] {
        assert!(caller_allowed("file.trash", Effect::Files, caller));
    }
}

#[test]
fn the_automation_switch_is_ui_only_and_the_denied_handler_is_never_reached() {
    let spec = Registry::new()
        .command("agent.set-enabled")
        .expect("the approved switch exists");
    assert_eq!(spec.effect, Effect::Settings);
    assert!(!spec.undo);
    let argument = spec.argument("enabled").expect("a required bool");
    assert_eq!(argument.kind, ArgKind::Bool);
    assert_eq!(argument.presence, Presence::Required);
    assert!(argument.default.is_none());

    assert!(caller_allowed(
        "agent.set-enabled",
        Effect::Settings,
        Caller::Ui
    ));
    for caller in [Caller::Hotkey, Caller::Cli, Caller::Agent] {
        assert!(!caller_allowed(
            "agent.set-enabled",
            Effect::Settings,
            caller
        ));
        for value in ["true", "false"] {
            let mut handler = Recorder::new(Outcome::Done);
            let outcome = Registry::new().dispatch(
                caller,
                &parse_invocation(&["agent.set-enabled", &format!("enabled={value}")]),
                &mut handler,
            );
            assert!(matches!(outcome, Outcome::Error(_)), "{caller} {value}");
            assert!(handler.seen.is_empty(), "{caller} reached a handler");
        }
    }

    let mut handler = Recorder::new(Outcome::Done);
    let outcome = Registry::new().dispatch(
        Caller::Ui,
        &parse_invocation(&["agent.set-enabled", "enabled=false"]),
        &mut handler,
    );
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(handler.seen.len(), 1);
    assert!(check(Caller::Ui, &["agent.set-enabled"]).is_err());
    assert!(check(Caller::Ui, &["agent.set-enabled", "enabled=maybe"]).is_err());
}

#[test]
fn preview_editing_commands_refuse_the_cli_and_agent_before_any_handler() {
    for command in PREVIEW_EDITING_COMMANDS {
        assert!(caller_allowed(command, Effect::View, Caller::Ui));
        assert!(caller_allowed(command, Effect::View, Caller::Hotkey));
        assert!(!caller_allowed(command, Effect::Files, Caller::Cli));
        assert!(!caller_allowed(command, Effect::Files, Caller::Agent));

        for caller in [Caller::Cli, Caller::Agent] {
            let mut handler = Recorder::new(Outcome::Done);
            let outcome =
                Registry::new().dispatch(caller, &parse_invocation(&[command]), &mut handler);
            assert!(
                matches!(outcome, Outcome::Error(_)),
                "{command} via {caller}"
            );
            assert!(handler.seen.is_empty(), "{command} reached a handler");
        }
    }

    let mut handler = Recorder::new(Outcome::Done);
    let outcome = Registry::new().dispatch(
        Caller::Ui,
        &parse_invocation(&["preview.edit"]),
        &mut handler,
    );
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(handler.seen.len(), 1);

    // Save As keeps its explicit path argument; no optional implicit variant.
    assert!(
        check(Caller::Ui, &["preview.save-as"]).is_err(),
        "preview.save-as must require its path"
    );
    assert!(check(Caller::Ui, &["preview.save-as", "path=/tmp/new.txt"]).is_ok());
}

#[test]
fn every_files_command_gets_exactly_one_dry_run_added_by_the_registry() {
    for spec in COMMANDS {
        let arguments = spec.arguments();
        assert!(
            spec.arguments.iter().all(|arg| arg.name != "dry-run"),
            "{} lists dry-run in its own table",
            spec.name
        );

        let dry_run = arguments
            .iter()
            .filter(|argument| argument.name == "dry-run")
            .copied()
            .collect::<Vec<&ArgSpec>>();

        if spec.effect == Effect::Files {
            assert_eq!(dry_run.len(), 1, "{} needs one dry-run", spec.name);
            let argument = dry_run[0];
            assert_eq!(argument.kind, ArgKind::Bool, "{}", spec.name);
            assert_eq!(argument.presence, Presence::Optional, "{}", spec.name);
            assert_eq!(argument.default, Some("false"), "{}", spec.name);
        } else {
            assert!(dry_run.is_empty(), "{} must not take dry-run", spec.name);
        }
    }
}

#[test]
fn undo_is_only_offered_on_files_commands() {
    for spec in COMMANDS {
        if spec.undo {
            assert_eq!(spec.effect, Effect::Files, "{}", spec.name);
        }
    }
}

#[test]
fn argument_names_are_unique_within_a_command() {
    for spec in COMMANDS {
        let mut seen = HashSet::new();
        for argument in spec.arguments() {
            assert!(
                seen.insert(argument.name),
                "{} repeats argument {}",
                spec.name,
                argument.name
            );
        }
    }
}

#[test]
fn argument_kinds_are_sane() {
    for spec in COMMANDS {
        for argument in spec.arguments() {
            match argument.kind {
                ArgKind::Int { min, max } => assert!(min <= max, "{}", spec.name),
                ArgKind::Number { min, max, step } => {
                    assert!(min <= max, "{}", spec.name);
                    if let Some(step) = step {
                        assert!(step > 0, "{}", spec.name);
                    }
                }
                ArgKind::Choice(values) => {
                    assert!(!values.is_empty(), "{}", spec.name);
                    let mut seen = HashSet::new();
                    for value in values {
                        assert!(seen.insert(value), "{} repeats choice {}", spec.name, value);
                    }
                }
                ArgKind::Path
                | ArgKind::Paths
                | ArgKind::TrashItems
                | ArgKind::Bool
                | ArgKind::Text => {}
            }
        }
    }
}

// -------------------------------------------------------------------------
// Argument kinds
// -------------------------------------------------------------------------

#[test]
fn path_must_be_absolute_and_stays_as_given() {
    let command = check(Caller::Ui, &["nav.go", "path=/tmp/a b"]).expect("valid path");
    assert_eq!(
        command.get("path"),
        Some(&Value::Path(PathBuf::from("/tmp/a b")))
    );

    for text in ["relative/path", "~/Documents", ""] {
        let token = format!("path={text}");
        let error = check(Caller::Ui, &["nav.go", token.as_str()]).expect_err("must refuse");
        assert_eq!(error.argument(), Some("path"));
        assert!(
            matches!(error.kind(), CommandErrorKind::BadValue { .. }),
            "{text}"
        );
        assert!(error.to_string().contains("absolute"));
    }
}

#[test]
fn repeated_paths_keys_make_a_list_in_input_order() {
    let command = check(
        Caller::Ui,
        &["file.copy", "paths=/tmp/a", "paths=/tmp/b", "to=/tmp/c"],
    )
    .expect("valid copy");
    assert_eq!(
        command.get("paths"),
        Some(&Value::Paths(vec![
            PathBuf::from("/tmp/a"),
            PathBuf::from("/tmp/b")
        ]))
    );
}

#[test]
fn trash_restore_takes_repeated_validated_item_uris() {
    let command = check(
        Caller::Cli,
        &[
            "trash.restore",
            "items=trash:///one.txt",
            "items=trash:///a%20b.txt",
        ],
    )
    .expect("the CLI may name explicit trash items");
    assert_eq!(
        command.get("items"),
        Some(&Value::TrashItems(vec![
            TrashUri::parse("trash:///one.txt").unwrap(),
            TrashUri::parse("trash:///a%20b.txt").unwrap(),
        ]))
    );
    assert_eq!(
        command.to_string(),
        "trash.restore items=trash:///one.txt items=trash:///a%20b.txt dry-run=false"
    );

    for bad in [
        "items=trash:///",
        "items=file:///tmp/a",
        "items=trash:///../a",
        "items=trash:///a%2Fb",
        "items=trash:///a%zz",
    ] {
        let error = check(Caller::Ui, &["trash.restore", bad]).expect_err("invalid item URI");
        assert_eq!(error.argument(), Some("items"));
        assert!(
            matches!(error.kind(), CommandErrorKind::BadValue { .. }),
            "{bad}"
        );
    }
}

#[test]
fn an_empty_trash_takes_only_its_registry_dry_run() {
    let command = check(Caller::Ui, &["trash.empty", "dry-run=true"]).expect("UI dry run");
    assert!(command.dry_run());
    let error = check(Caller::Ui, &["trash.empty", "path=/"]).expect_err("no path argument");
    assert!(matches!(error.kind(), CommandErrorKind::UnknownArgument));
}

#[test]
fn selected_permanent_deletion_requires_valid_trash_item_uris() {
    let command = check(
        Caller::Ui,
        &[
            "trash.delete-permanently",
            "items=trash:///one.txt",
            "items=trash:///a%20b.txt",
            "dry-run=true",
        ],
    )
    .expect("the UI may name explicit validated Trash items");
    assert_eq!(
        command.get("items"),
        Some(&Value::TrashItems(vec![
            TrashUri::parse("trash:///one.txt").unwrap(),
            TrashUri::parse("trash:///a%20b.txt").unwrap(),
        ]))
    );
    assert!(command.dry_run());

    // There is no empty-list meaning, no implicit all-items or root fallback.
    let error = check(Caller::Ui, &["trash.delete-permanently"]).expect_err("items are required");
    assert_eq!(error.argument(), Some("items"));
    assert!(matches!(error.kind(), CommandErrorKind::MissingArgument));

    for bad in [
        "items=",
        "items=trash:///",
        "items=file:///tmp/a",
        "items=trash:///../a",
        "items=trash:///a%2Fb",
        "items=trash:///a%zz",
    ] {
        let error = check(Caller::Ui, &["trash.delete-permanently", bad])
            .expect_err("only a validated item URI is accepted");
        assert_eq!(error.command(), Some("trash.delete-permanently"));
        assert_eq!(error.argument(), Some("items"));
        assert!(matches!(error.kind(), CommandErrorKind::BadValue { .. }));
    }

    let error = check(
        Caller::Ui,
        &["trash.delete-permanently", "items=trash:///a.txt", "path=/"],
    )
    .expect_err("no extra bypass argument");
    assert!(matches!(error.kind(), CommandErrorKind::UnknownArgument));
}

#[test]
fn int_accepts_the_range_and_rejects_bad_and_out_of_range_values() {
    let command = check(Caller::Ui, &["tab.focus", "index=0"]).expect("zero is allowed");
    assert_eq!(command.get("index"), Some(&Value::Int(0)));

    let command = check(Caller::Ui, &["tab.focus", "index=12"]).expect("twelve is allowed");
    assert_eq!(command.get("index"), Some(&Value::Int(12)));

    let error = check(Caller::Ui, &["tab.focus", "index=-1"]).expect_err("negative is refused");
    assert!(matches!(error.kind(), CommandErrorKind::OutOfRange { .. }));

    let error = check(Caller::Ui, &["tab.focus", "index=two"]).expect_err("text is refused");
    assert!(matches!(error.kind(), CommandErrorKind::BadValue { .. }));
}

#[test]
fn scale_accepts_the_step_edges_and_refuses_off_step_and_out_of_range() {
    for (text, hundredths) in [
        ("0.5", 50),
        ("0.50", 50),
        ("0.55", 55),
        ("1", 100),
        ("1.0", 100),
    ] {
        let token = format!("scale={text}");
        let command = check(Caller::Ui, &["view.set-scale", token.as_str()])
            .unwrap_or_else(|error| panic!("{text} should be valid: {error}"));
        assert_eq!(
            command.get("scale"),
            Some(&Value::Number(Number::from_hundredths(hundredths))),
            "{text}"
        );
    }

    let error = check(Caller::Ui, &["view.set-scale", "scale=0.57"]).expect_err("off step");
    assert!(
        matches!(error.kind(), CommandErrorKind::OffStep { .. }),
        "0.57 should be off step"
    );
    assert!(error.to_string().contains("off the 0.05 step"));

    for text in ["0.45", "1.05"] {
        let token = format!("scale={text}");
        let error = check(Caller::Ui, &["view.set-scale", token.as_str()]).expect_err("range");
        assert!(
            matches!(error.kind(), CommandErrorKind::OutOfRange { .. }),
            "{text} should be out of range"
        );
    }
}

#[test]
fn opacity_is_a_number_without_a_step() {
    let command = check(Caller::Ui, &["view.set-opacity", "opacity=0.85"]).expect("valid opacity");
    assert_eq!(
        command.get("opacity"),
        Some(&Value::Number(Number::from_hundredths(85)))
    );

    let error = check(Caller::Ui, &["view.set-opacity", "opacity=0.851"]).expect_err("too precise");
    assert!(matches!(error.kind(), CommandErrorKind::BadValue { .. }));

    let error = check(Caller::Ui, &["view.set-opacity", "opacity=1.1"]).expect_err("too large");
    assert!(matches!(error.kind(), CommandErrorKind::OutOfRange { .. }));
}

#[test]
fn bool_accepts_true_and_false_only() {
    let command =
        check(Caller::Ui, &["file.trash", "paths=/tmp/x", "dry-run=true"]).expect("valid dry run");
    assert_eq!(command.get("dry-run"), Some(&Value::Bool(true)));
    assert!(command.dry_run());

    let command = check(Caller::Ui, &["file.trash", "paths=/tmp/x", "dry-run=false"])
        .expect("valid ordinary run");
    assert_eq!(command.get("dry-run"), Some(&Value::Bool(false)));
    assert!(!command.dry_run());

    let error = check(Caller::Ui, &["file.trash", "paths=/tmp/x", "dry-run=yes"])
        .expect_err("yes is not a bool");
    assert!(matches!(error.kind(), CommandErrorKind::BadValue { .. }));
}

#[test]
fn a_files_command_defaults_dry_run_to_false() {
    let command = check(Caller::Ui, &["file.trash", "paths=/tmp/x"]).expect("valid");
    assert_eq!(command.get("dry-run"), Some(&Value::Bool(false)));
}

#[test]
fn group_open_takes_required_repeated_paths_for_every_caller() {
    let spec = COMMANDS
        .iter()
        .find(|spec| spec.name == "group.open")
        .expect("the reviewed group.open command");
    assert_eq!(spec.effect, Effect::View);
    assert!(!spec.undo);
    assert_eq!(spec.arguments.len(), 1);
    assert_eq!(spec.arguments[0].name, "paths");
    assert!(matches!(spec.arguments[0].kind, ArgKind::Paths));
    assert!(matches!(spec.arguments[0].presence, Presence::Required));

    for caller in [Caller::Ui, Caller::Hotkey, Caller::Cli, Caller::Agent] {
        let command = check(caller, &["group.open", "paths=/tmp/a", "paths=/tmp/b"])
            .expect("every caller may name explicit folders");
        assert_eq!(
            command.get("paths"),
            Some(&Value::Paths(vec![
                PathBuf::from("/tmp/a"),
                PathBuf::from("/tmp/b")
            ]))
        );
        assert!(
            check(caller, &["group.open"]).is_err(),
            "{caller} must supply the paths"
        );
    }
}

#[test]
fn text_must_not_be_empty() {
    let command = check(Caller::Ui, &["group.rename", "name=Trips"]).expect("valid name");
    assert_eq!(command.get("name"), Some(&Value::Text("Trips".to_string())));

    let error = check(Caller::Ui, &["group.rename", "name="]).expect_err("empty is refused");
    assert!(matches!(error.kind(), CommandErrorKind::BadValue { .. }));
    assert!(error.to_string().contains("not be empty"));
}

#[test]
fn choice_must_be_one_of_the_fixed_list() {
    let command =
        check(Caller::Ui, &["view.toggle-sidebar", "side=left"]).expect("left is allowed");
    assert_eq!(command.get("side"), Some(&Value::Choice("left")));

    let error =
        check(Caller::Ui, &["view.toggle-sidebar", "side=middle"]).expect_err("middle is not");
    assert!(matches!(error.kind(), CommandErrorKind::BadValue { .. }));
    assert!(error.to_string().contains("left, right"));
}

#[test]
fn the_sort_scope_defaults_to_folder() {
    let command =
        check(Caller::Ui, &["view.set-sort", "by=name", "order=ascending"]).expect("valid sort");
    assert_eq!(command.get("scope"), Some(&Value::Choice("folder")));

    let command = check(
        Caller::Ui,
        &[
            "view.set-sort",
            "by=name",
            "order=ascending",
            "scope=global",
        ],
    )
    .expect("valid sort");
    assert_eq!(command.get("scope"), Some(&Value::Choice("global")));
}

#[test]
fn clear_folder_sort_is_a_plain_settings_command_for_every_caller() {
    let sort =
        check(Caller::Ui, &["view.set-sort", "by=name", "order=ascending"]).expect("valid sort");
    for caller in [Caller::Ui, Caller::Hotkey, Caller::Cli, Caller::Agent] {
        let clear = check(caller, &["view.clear-folder-sort"]).expect("clear-folder-sort is valid");
        assert_eq!(clear.effect(), Effect::Settings);
        assert_eq!(clear.effect(), sort.effect());
        assert!(clear.arguments().is_empty());
    }
}

#[test]
fn number_values_are_hundredths_and_print_with_two_places() {
    assert_eq!(Number::from_hundredths(55).hundredths(), 55);
    assert_eq!(Number::from_hundredths(55).to_string(), "0.55");
    assert_eq!(format_hundredths(5), "0.05");
    assert_eq!(format_hundredths(100), "1.00");
    assert_eq!(format_hundredths(0), "0.00");
}

// -------------------------------------------------------------------------
// Errors
// -------------------------------------------------------------------------

#[test]
fn missing_required_argument_is_named() {
    let error = check(Caller::Ui, &["nav.go"]).expect_err("path is required");
    assert_eq!(error.command(), Some("nav.go"));
    assert_eq!(error.argument(), Some("path"));
    assert!(matches!(error.kind(), CommandErrorKind::MissingArgument));
    assert_eq!(
        error.to_string(),
        "command `nav.go`: argument `path`: missing"
    );
}

#[test]
fn unknown_argument_is_named() {
    let error = check(Caller::Ui, &["nav.go", "path=/tmp", "pth=/tmp"])
        .expect_err("pth is not an argument");
    assert_eq!(error.command(), Some("nav.go"));
    assert_eq!(error.argument(), Some("pth"));
    assert!(matches!(error.kind(), CommandErrorKind::UnknownArgument));
    assert_eq!(
        error.to_string(),
        "command `nav.go`: unknown argument `pth`"
    );
}

#[test]
fn a_single_valued_argument_may_not_repeat() {
    let error =
        check(Caller::Ui, &["nav.go", "path=/tmp", "path=/var"]).expect_err("path is single");
    assert_eq!(error.argument(), Some("path"));
    assert!(matches!(error.kind(), CommandErrorKind::DuplicateArgument));
    assert!(error.to_string().contains("given more than once"));
}

#[test]
fn unknown_command_suggests_the_closest_name() {
    let error = check(Caller::Ui, &["tab.clsoe"]).expect_err("typo");
    assert_eq!(error.command(), Some("tab.clsoe"));
    assert_eq!(error.argument(), None);
    assert_eq!(error.suggestion(), Some("tab.close"));
    assert!(matches!(
        error.kind(),
        CommandErrorKind::UnknownCommand {
            suggestion: Some("tab.close")
        }
    ));
    assert_eq!(
        error.to_string(),
        "unknown command `tab.clsoe`; did you mean `tab.close`?"
    );
}

#[test]
fn a_distant_typo_gets_no_suggestion() {
    let error = check(Caller::Ui, &["totally.bogus"]).expect_err("unknown");
    assert_eq!(error.suggestion(), None);
    assert_eq!(error.to_string(), "unknown command `totally.bogus`");
}

#[test]
fn a_known_command_is_not_an_unknown_command() {
    assert!(check(Caller::Ui, &["tab.close"]).is_ok());
}

// -------------------------------------------------------------------------
// key=value parsing
// -------------------------------------------------------------------------

#[test]
fn tokens_parse_into_a_command_and_arguments() {
    let invocation = parse_invocation(&["nav.go", "path=/home/kyle"]);
    assert_eq!(invocation.name(), "nav.go");
    assert_eq!(
        invocation.arguments(),
        &[("path".to_string(), "/home/kyle".to_string())]
    );
}

#[test]
fn repeated_keys_are_kept_in_order() {
    let invocation = parse_invocation(&["file.copy", "paths=/tmp/a", "paths=/tmp/b", "to=/tmp/c"]);
    assert_eq!(invocation.arguments().len(), 3);
    assert_eq!(invocation.arguments()[0].1, "/tmp/a");
    assert_eq!(invocation.arguments()[1].1, "/tmp/b");
    assert_eq!(invocation.arguments()[2].1, "/tmp/c");
}

#[test]
fn a_value_may_contain_equals_and_spaces() {
    let invocation = parse_invocation(&["group.rename", "name=a=b c"]);
    assert_eq!(invocation.arguments()[0].0, "name");
    assert_eq!(invocation.arguments()[0].1, "a=b c");

    let command = check(Caller::Ui, &["group.rename", "name=a=b c"]).expect("valid text");
    assert_eq!(command.get("name"), Some(&Value::Text("a=b c".to_string())));
}

#[test]
fn a_token_without_equals_is_an_error() {
    let tokens = ["nav.go".to_string(), "path".to_string()];
    let error = Invocation::parse(&tokens).expect_err("no equals");
    assert!(matches!(
        error.kind(),
        CommandErrorKind::MalformedToken { .. }
    ));
    assert!(error.to_string().contains("expected key=value"));
}

#[test]
fn an_empty_argument_name_is_an_error() {
    let tokens = ["nav.go".to_string(), "=/tmp".to_string()];
    let error = Invocation::parse(&tokens).expect_err("empty key");
    assert!(matches!(
        error.kind(),
        CommandErrorKind::EmptyArgumentName { .. }
    ));
}

#[test]
fn no_tokens_is_a_missing_command() {
    let error = Invocation::parse(&[]).expect_err("no command");
    assert!(matches!(error.kind(), CommandErrorKind::MissingCommand));
    assert_eq!(error.to_string(), "missing command name");
}

// -------------------------------------------------------------------------
// The caller rules
// -------------------------------------------------------------------------

#[test]
fn ui_and_hotkey_may_fill_files_paths_from_context() {
    for caller in [Caller::Ui, Caller::Hotkey] {
        let command = check(caller, &["file.trash"]).expect("context is allowed");
        assert!(command.get("paths").is_none());
        assert_eq!(command.get("dry-run"), Some(&Value::Bool(false)));
    }
}

#[test]
fn cli_and_agent_may_not_fill_files_paths_from_context() {
    for caller in [Caller::Cli, Caller::Agent] {
        let error = check(caller, &["file.trash"]).expect_err("no context for this caller");
        assert_eq!(error.command(), Some("file.trash"));
        assert_eq!(error.argument(), Some("paths"));
        assert!(matches!(
            error.kind(),
            CommandErrorKind::ForbiddenFromContext { caller: actual } if *actual == caller
        ));
        let text = error.to_string();
        assert!(text.contains(caller.as_str()));
        assert!(text.contains("may not fill a Files path from context"));
    }
}

#[test]
fn cli_and_agent_must_give_every_files_context_path() {
    for caller in [Caller::Cli, Caller::Agent] {
        assert!(check(caller, &["file.copy", "to=/tmp/c"]).is_err());
        assert!(check(caller, &["file.move", "to=/tmp/c"]).is_err());
        assert!(check(caller, &["file.new-folder", "name=New"]).is_err());
        assert!(check(caller, &["file.rename", "name=New"]).is_err());
    }
}

#[test]
fn every_caller_may_give_explicit_files_paths() {
    for caller in [Caller::Ui, Caller::Hotkey, Caller::Cli, Caller::Agent] {
        assert!(check(caller, &["file.copy", "paths=/tmp/a", "to=/tmp/c"]).is_ok());
        assert!(check(caller, &["file.trash", "paths=/tmp/x"]).is_ok());
        assert!(check(caller, &["file.new-folder", "name=New", "in=/tmp"],).is_ok());
    }
}

#[test]
fn the_context_rule_applies_only_to_files_commands() {
    for caller in [Caller::Cli, Caller::Agent] {
        assert!(check(caller, &["app.disk-usage"]).is_ok());
        assert!(check(caller, &["pins.add"]).is_ok());
    }
}

#[test]
fn context_rules_are_pure_functions_over_every_caller() {
    let registry = Registry::new();
    let trash = registry.command("file.trash").expect("catalogue");
    let paths = trash.argument("paths").expect("paths");

    assert!(may_use_context(Caller::Ui, trash.effect, paths));
    assert!(may_use_context(Caller::Hotkey, trash.effect, paths));
    assert!(!may_use_context(Caller::Cli, trash.effect, paths));
    assert!(!may_use_context(Caller::Agent, trash.effect, paths));

    assert!(!must_supply(Caller::Ui, trash.effect, paths));
    assert!(!must_supply(Caller::Hotkey, trash.effect, paths));
    assert!(must_supply(Caller::Cli, trash.effect, paths));
    assert!(must_supply(Caller::Agent, trash.effect, paths));

    let disk_usage = registry.command("app.disk-usage").expect("catalogue");
    let path = disk_usage.argument("path").expect("path");
    assert!(may_use_context(Caller::Cli, disk_usage.effect, path));
    assert!(may_use_context(Caller::Agent, disk_usage.effect, path));
    assert!(!must_supply(Caller::Cli, disk_usage.effect, path));
    assert!(!may_use_context(
        Caller::Cli,
        disk_usage.effect,
        &DRY_RUN_ARG
    ));

    let nav = registry.command("nav.go").expect("catalogue");
    let required = nav.argument("path").expect("path");
    for caller in [Caller::Ui, Caller::Hotkey, Caller::Cli, Caller::Agent] {
        assert!(must_supply(caller, nav.effect, required));
    }
}

// -------------------------------------------------------------------------
// The dispatcher
// -------------------------------------------------------------------------

struct Recorder {
    seen: Vec<CheckedCommand>,
    outcome: Outcome,
}

impl Recorder {
    fn new(outcome: Outcome) -> Self {
        Self {
            seen: Vec::new(),
            outcome,
        }
    }
}

impl Handler for Recorder {
    fn handle(&mut self, command: &CheckedCommand) -> Outcome {
        self.seen.push(command.clone());
        self.outcome.clone()
    }
}

#[test]
fn a_valid_call_reaches_the_handler_once() {
    let mut handler = Recorder::new(Outcome::Done);
    let outcome = Registry::new().dispatch(
        Caller::Ui,
        &parse_invocation(&["nav.go", "path=/tmp"]),
        &mut handler,
    );

    assert_eq!(outcome, Outcome::Done);
    assert_eq!(handler.seen.len(), 1);
    assert_eq!(handler.seen[0].name(), "nav.go");
    assert_eq!(handler.seen[0].effect(), Effect::View);
}

#[test]
fn an_invalid_call_never_reaches_the_handler() {
    let mut handler = Recorder::new(Outcome::Done);
    let outcome =
        Registry::new().dispatch(Caller::Ui, &parse_invocation(&["nav.go"]), &mut handler);

    assert!(matches!(outcome, Outcome::Error(_)));
    assert!(handler.seen.is_empty());
}

#[test]
fn a_forbidden_call_never_reaches_the_handler() {
    let mut handler = Recorder::new(Outcome::Done);
    let outcome = Registry::new().dispatch(
        Caller::Cli,
        &parse_invocation(&["file.trash"]),
        &mut handler,
    );

    assert!(matches!(outcome, Outcome::Error(_)));
    assert!(handler.seen.is_empty());
}

#[test]
fn the_handler_outcome_is_returned_unchanged() {
    let plan = vec!["trash /tmp/x".to_string()];
    let mut handler = Recorder::new(Outcome::DryRun(plan.clone()));
    let outcome = Registry::new().dispatch(
        Caller::Cli,
        &parse_invocation(&["file.trash", "paths=/tmp/x", "dry-run=true"]),
        &mut handler,
    );

    assert_eq!(outcome, Outcome::DryRun(plan));
    assert_eq!(handler.seen.len(), 1);
    assert!(handler.seen[0].dry_run());
}

// -------------------------------------------------------------------------
// Printing
// -------------------------------------------------------------------------

#[test]
fn a_checked_command_prints_in_catalogue_order() {
    let command = check(
        Caller::Ui,
        &[
            "view.set-sort",
            "scope=global",
            "order=descending",
            "by=size",
        ],
    )
    .expect("valid sort");
    assert_eq!(
        command.to_string(),
        "view.set-sort by=size order=descending scope=global"
    );
}

#[test]
fn paths_print_once_per_key() {
    let command =
        check(Caller::Ui, &["file.copy", "paths=/a", "paths=/b", "to=/c"]).expect("valid copy");
    assert_eq!(
        command.to_string(),
        "file.copy paths=/a paths=/b to=/c dry-run=false"
    );
}

#[test]
fn usage_lists_arguments_in_catalogue_order() {
    let registry = Registry::new();

    let sort = registry.command("view.set-sort").expect("catalogue");
    assert_eq!(
        sort.usage(),
        "view.set-sort by (name|modified|size|type) order (ascending|descending) scope? (folder|global; default folder)"
    );

    let trash = registry.command("file.trash").expect("catalogue");
    assert_eq!(
        trash.usage(),
        "file.trash paths? (paths, from selection) dry-run? (bool; default false)"
    );

    let quit = registry.command("app.quit").expect("catalogue");
    assert_eq!(quit.usage(), "app.quit");
}
