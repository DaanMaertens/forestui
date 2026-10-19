//! The Start Work Settings dialog: the global Start work settings, or one
//! repository's overrides of them, edited in place of `settings.json`.
//!
//! Both modes share one field table, [`FIELDS`]. A field is a path into the
//! `start_work` section and a kind, and everything else — loading, the
//! inherited placeholder, validation, the global save and the repository's
//! overrides — follows from those two, so a new field is a row there and a
//! `FOCUS_*` constant beside it.

use super::{ModalEffect, ModalOutcome, ModalResult, cycle_focus, edit_input, is_escape};
use crate::models::StartWorkSettings;
use crate::repo_settings::{ConfigFile, OverrideChange, RepoKey, set_path};
use crate::ui::widgets::TextInput;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

/// Which part of the dialog a field is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Listing,
    Naming,
    Suggestion,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Listing, Section::Naming, Section::Suggestion];

    pub fn title(self) -> &'static str {
        match self {
            Section::Listing => "LISTING",
            Section::Naming => "NAMING",
            Section::Suggestion => "SINGLE SESSION NAME SUGGESTION",
        }
    }
}

/// What a field holds, which decides how it is edited and written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string, as typed.
    Text,
    /// A list of strings, edited comma-separated.
    List,
    /// A positive whole number.
    Count,
    /// `mine` or `all`.
    Scope,
    /// On or off; a checkbox in the global dialog.
    Toggle,
}

impl Kind {
    /// The values a choice cycles through, as written and as shown.
    fn choices(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Kind::Scope => &[("mine", "Mine"), ("all", "Everyone")],
            Kind::Toggle => &[("true", "On"), ("false", "Off")],
            Kind::Text | Kind::List | Kind::Count => &[],
        }
    }

    pub fn is_choice(self) -> bool {
        !self.choices().is_empty()
    }
}

/// One field: where it lives in `start_work`, what it holds, and where the
/// dialog draws it — a grid per section, `row` naming the row (the work
/// mode, for naming) and `column` the slot it takes in it.
#[derive(Debug)]
pub struct Field {
    pub path: &'static [&'static str],
    pub kind: Kind,
    pub section: Section,
    pub row: &'static str,
    /// The placeholders the row's patterns accept, under its label.
    pub hint: &'static str,
    pub column: usize,
    pub label: &'static str,
}

const NUMBER_SLUG: &str = "{number} {slug}";
const SLUG: &str = "{slug}";

const fn field(
    path: &'static [&'static str],
    kind: Kind,
    section: Section,
    (row, hint): (&'static str, &'static str),
    column: usize,
    label: &'static str,
) -> Field {
    Field {
        path,
        kind,
        section,
        row,
        hint,
        column,
        label,
    }
}

/// A field whose section has no row labels.
const NO_ROW: (&str, &str) = ("", "");
const ISSUE: (&str, &str) = ("issue", NUMBER_SLUG);
const FEATURE: (&str, &str) = ("feature", NUMBER_SLUG);
const SESSION: (&str, &str) = ("single session", SLUG);
const PR_REVIEW: (&str, &str) = ("pr-review", NUMBER_SLUG);
const PREFIX: &str = "Branch prefix";
const NAME: &str = "Worktree name";
const BRANCH: &str = "Branch name, after the prefix";

/// Every field, each at its `FOCUS_*` index, which is its place in the
/// focus ring and on screen.
pub const FIELDS: [Field; 16] = {
    use Kind::*;
    use Section::*;
    type M = StartWorkSettingsModal;
    const UNSET: Field = field(&[], Text, Listing, NO_ROW, 0, "");
    let mut fields = [UNSET; 16];
    fields[M::FOCUS_SCOPE] = field(&["scope"], Scope, Listing, NO_ROW, 0, "Default scope");
    fields[M::FOCUS_FEATURE_LABELS] = field(
        &["feature_labels"],
        List,
        Listing,
        NO_ROW,
        1,
        "Feature labels, comma-separated",
    );
    fields[M::FOCUS_ISSUE_PREFIX] =
        field(&["issue", "branch_prefix"], Text, Naming, ISSUE, 0, PREFIX);
    fields[M::FOCUS_ISSUE_NAME] = field(&["issue", "name_pattern"], Text, Naming, ISSUE, 1, NAME);
    fields[M::FOCUS_ISSUE_BRANCH] =
        field(&["issue", "branch_pattern"], Text, Naming, ISSUE, 2, BRANCH);
    fields[M::FOCUS_FEATURE_PREFIX] = field(
        &["feature", "branch_prefix"],
        Text,
        Naming,
        FEATURE,
        0,
        PREFIX,
    );
    fields[M::FOCUS_FEATURE_NAME] =
        field(&["feature", "name_pattern"], Text, Naming, FEATURE, 1, NAME);
    fields[M::FOCUS_FEATURE_BRANCH] = field(
        &["feature", "branch_pattern"],
        Text,
        Naming,
        FEATURE,
        2,
        BRANCH,
    );
    fields[M::FOCUS_SESSION_PREFIX] = field(
        &["session", "branch_prefix"],
        Text,
        Naming,
        SESSION,
        0,
        PREFIX,
    );
    fields[M::FOCUS_SESSION_NAME] =
        field(&["session", "name_pattern"], Text, Naming, SESSION, 1, NAME);
    fields[M::FOCUS_SESSION_BRANCH] = field(
        &["session", "branch_pattern"],
        Text,
        Naming,
        SESSION,
        2,
        BRANCH,
    );
    fields[M::FOCUS_PR_REVIEW_NAME] = field(
        &["pr_review", "name_pattern"],
        Text,
        Naming,
        PR_REVIEW,
        1,
        NAME,
    );
    fields[M::FOCUS_NAMING_ENABLED] = field(
        &["session_naming", "enabled"],
        Toggle,
        Suggestion,
        NO_ROW,
        0,
        "Suggest names",
    );
    fields[M::FOCUS_NAMING_MODEL] = field(
        &["session_naming", "model"],
        Text,
        Suggestion,
        NO_ROW,
        1,
        "Model",
    );
    fields[M::FOCUS_NAMING_TIMEOUT] = field(
        &["session_naming", "timeout_secs"],
        Count,
        Suggestion,
        NO_ROW,
        2,
        "Timeout, seconds",
    );
    fields[M::FOCUS_NAMING_TOOLS] = field(
        &["session_naming", "allowed_tools"],
        List,
        Suggestion,
        NO_ROW,
        3,
        "Allowed tools, comma-separated",
    );
    fields
};

// A `FOCUS_*` constant without its row would leave an unset field, with no
// path to read or write, silently in the ring.
const _: () = {
    let mut i = 0;
    while i < FIELDS.len() {
        assert!(!FIELDS[i].path.is_empty(), "a FIELDS row is unset");
        i += 1;
    }
};

/// A field as the dialog holds it.
#[derive(Debug, Clone)]
pub enum FieldState {
    /// Empty inherits, in the repository dialog.
    Text(TextInput),
    /// An index into the kind's choices; `None` inherits.
    Choice(Option<usize>),
}

/// Which settings the dialog edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Global,
    /// One repository's overrides. `problem` says why its file cannot be
    /// saved over; such a dialog offers only Cancel and the editor.
    Repository {
        key: RepoKey,
        problem: Option<String>,
    },
}

#[derive(Debug)]
pub struct StartWorkSettingsModal {
    pub target: Target,
    pub fields: Vec<FieldState>,
    /// Each field as it opened, so a repository save writes only what
    /// changed and leaves the rest of the file as it was.
    opened: Vec<Option<String>>,
    /// The global settings: what a global save starts from, and what an
    /// inherited field shows.
    global: StartWorkSettings,
    /// One of the `FOCUS_*` constants; see [`StartWorkSettingsModal`].
    pub focus: usize,
}

impl StartWorkSettingsModal {
    /// The focus ring, in the order `ui/modals.rs` draws it: the fields,
    /// each placed in [`FIELDS`] by its constant, then the buttons. A new
    /// field takes the next number and pushes the buttons along.
    pub const FOCUS_SCOPE: usize = 0;
    pub const FOCUS_FEATURE_LABELS: usize = 1;
    pub const FOCUS_ISSUE_PREFIX: usize = 2;
    pub const FOCUS_ISSUE_NAME: usize = 3;
    pub const FOCUS_ISSUE_BRANCH: usize = 4;
    pub const FOCUS_FEATURE_PREFIX: usize = 5;
    pub const FOCUS_FEATURE_NAME: usize = 6;
    pub const FOCUS_FEATURE_BRANCH: usize = 7;
    pub const FOCUS_SESSION_PREFIX: usize = 8;
    pub const FOCUS_SESSION_NAME: usize = 9;
    pub const FOCUS_SESSION_BRANCH: usize = 10;
    pub const FOCUS_PR_REVIEW_NAME: usize = 11;
    pub const FOCUS_NAMING_ENABLED: usize = 12;
    pub const FOCUS_NAMING_MODEL: usize = 13;
    pub const FOCUS_NAMING_TIMEOUT: usize = 14;
    pub const FOCUS_NAMING_TOOLS: usize = 15;
    pub const FOCUS_SAVE: usize = FIELDS.len();
    pub const FOCUS_CANCEL: usize = FIELDS.len() + 1;
    /// Repository dialog only.
    pub const FOCUS_EDIT_FILE: usize = FIELDS.len() + 2;

    /// The global settings, every field filled.
    pub fn global(settings: &StartWorkSettings) -> Self {
        let values = to_json(settings);
        Self::new(Target::Global, settings, |field| lookup(&values, field))
    }

    /// One repository's overrides, from `file` — its `settings.json` as
    /// read: `Ok(None)` when there is none yet, `Err` when it cannot be
    /// read or parsed, which leaves nothing to save over.
    pub fn repository(
        key: RepoKey,
        global: &StartWorkSettings,
        file: Result<Option<Value>, String>,
    ) -> Self {
        let (problem, file) = match file {
            Ok(file) => (None, file.unwrap_or(Value::Null)),
            Err(problem) => (Some(problem), Value::Null),
        };
        let section = file.get("start_work").cloned().unwrap_or(Value::Null);
        let broken = problem.is_some();
        let mut modal = Self::new(Target::Repository { key, problem }, global, |field| {
            lookup(&section, field)
        });
        if broken {
            modal.focus = Self::FOCUS_EDIT_FILE;
        }
        modal
    }

    fn new(
        target: Target,
        global: &StartWorkSettings,
        value: impl Fn(&Field) -> Option<String>,
    ) -> Self {
        let inherited = to_json(global);
        let fields = FIELDS
            .iter()
            .map(|field| {
                let shown = value(field);
                if field.kind.is_choice() {
                    let index = shown.and_then(|v| choice_index(field.kind, &v));
                    return FieldState::Choice(index);
                }
                // An empty field inherits, so an empty value the file sets
                // by hand would read as inherited; the placeholder says it
                // is not. It stays in the file unless the field is edited.
                let placeholder = match (&target, shown.as_deref()) {
                    (Target::Global, _) => String::new(),
                    (Target::Repository { .. }, Some("")) => {
                        "(set empty in settings.json)".to_string()
                    }
                    (Target::Repository { .. }, _) => lookup(&inherited, field).unwrap_or_default(),
                };
                FieldState::Text(
                    TextInput::new(shown.unwrap_or_default()).with_placeholder(placeholder),
                )
            })
            .collect::<Vec<_>>();
        let mut modal = Self {
            target,
            opened: Vec::new(),
            fields,
            global: global.clone(),
            focus: Self::FOCUS_SCOPE,
        };
        modal.opened = (0..FIELDS.len()).map(|i| modal.value(i)).collect();
        modal
    }

    pub fn is_repository(&self) -> bool {
        matches!(self.target, Target::Repository { .. })
    }

    /// Why this repository's file cannot be saved over, if it cannot.
    pub fn problem(&self) -> Option<&str> {
        match &self.target {
            Target::Repository { problem, .. } => problem.as_deref(),
            Target::Global => None,
        }
    }

    /// Field `index` as written: `None` inherits.
    fn value(&self, index: usize) -> Option<String> {
        match (&self.fields[index], FIELDS[index].kind) {
            (FieldState::Text(input), _) => {
                // Blank is empty: spaces alone are no override.
                let value = input.value();
                (!value.trim().is_empty() || !self.is_repository()).then(|| value.to_string())
            }
            (FieldState::Choice(choice), kind) => choice
                .and_then(|i| kind.choices().get(i))
                .map(|(v, _)| v.to_string()),
        }
    }

    /// What a choice field shows: its value, or what it inherits.
    pub fn choice_label(&self, index: usize) -> String {
        let field = &FIELDS[index];
        let label = |value: &str| {
            field
                .kind
                .choices()
                .iter()
                .find(|(v, _)| *v == value)
                .map_or("", |(_, label)| *label)
        };
        match self.value(index) {
            Some(value) => label(&value).to_string(),
            None => {
                let inherited = lookup(&to_json(&self.global), field).unwrap_or_default();
                format!("Inherit ({})", label(&inherited))
            }
        }
    }

    /// Whether a toggle is on, for the global dialog's checkbox.
    pub fn is_on(&self, index: usize) -> bool {
        self.value(index).as_deref() == Some("true")
    }

    /// Why field `index` cannot be saved as it stands.
    pub fn error(&self, index: usize) -> Option<&'static str> {
        let value = self.value(index);
        match FIELDS[index].kind {
            Kind::Count => match value {
                None => None,
                Some(value) => match value.trim().parse::<u64>() {
                    Ok(n) if n > 0 => None,
                    _ => Some("must be a positive whole number"),
                },
            },
            _ => None,
        }
    }

    pub fn can_save(&self) -> bool {
        self.problem().is_none() && (0..FIELDS.len()).all(|i| self.error(i).is_none())
    }

    /// The focus ring as it stands: a broken repository file leaves only
    /// the editor and Cancel.
    pub fn ring(&self) -> Vec<usize> {
        let edit = self.is_repository().then_some(Self::FOCUS_EDIT_FILE);
        if self.problem().is_some() {
            return edit.into_iter().chain([Self::FOCUS_CANCEL]).collect();
        }
        (0..FIELDS.len())
            .chain([Self::FOCUS_SAVE, Self::FOCUS_CANCEL])
            .chain(edit)
            .collect()
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.ring().contains(&index) {
            self.focus = index;
        }
    }

    fn cycle_choice(&mut self, forward: bool) {
        let kind = FIELDS[self.focus].kind;
        let repository = self.is_repository();
        let Some(FieldState::Choice(choice)) = self.fields.get_mut(self.focus) else {
            return;
        };
        // Inherit is one more stop on the cycle, in the repository dialog.
        let stops = kind.choices().len() + usize::from(repository);
        let position = match (*choice, repository) {
            (None, _) => 0,
            (Some(i), true) => i + 1,
            (Some(i), false) => i,
        };
        let next = if forward {
            (position + 1) % stops
        } else {
            (position + stops - 1) % stops
        };
        *choice = match repository {
            true => next.checked_sub(1),
            false => Some(next),
        };
    }

    /// The global settings as the fields have them.
    fn to_settings(&self) -> Option<StartWorkSettings> {
        let mut values = to_json(&self.global);
        for (index, field) in FIELDS.iter().enumerate() {
            let value = self.value(index).and_then(|v| parse(field.kind, &v))?;
            set_path(&mut values, field.path, Some(value));
        }
        serde_json::from_value(values).ok()
    }

    /// The overrides that changed since the dialog opened: each path, and
    /// what to write there — `None` to inherit again.
    pub fn changes(&self) -> Vec<OverrideChange> {
        FIELDS
            .iter()
            .enumerate()
            .filter_map(|(index, field)| {
                let value = self.value(index);
                (value != self.opened[index])
                    .then(|| (field.path, value.and_then(|v| parse(field.kind, &v))))
            })
            .collect()
    }

    fn save(&self) -> ModalOutcome {
        if !self.can_save() {
            return ModalOutcome::None;
        }
        match &self.target {
            Target::Global => match self.to_settings() {
                Some(settings) => {
                    ModalOutcome::Submit(ModalResult::StartWorkSettingsSaved(Box::new(settings)))
                }
                None => ModalOutcome::None,
            },
            Target::Repository { key, .. } => {
                ModalOutcome::Submit(ModalResult::RepositoryStartWorkSaved {
                    key: key.clone(),
                    changes: self.changes(),
                })
            }
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        if let Some(FieldState::Text(input)) = self.fields.get_mut(self.focus)
            && edit_input(input, key)
        {
            return ModalOutcome::None;
        }
        let is_choice = self.focus < FIELDS.len() && FIELDS[self.focus].kind.is_choice();
        if is_choice {
            match key.code {
                KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
                    self.cycle_choice(true);
                    return ModalOutcome::None;
                }
                KeyCode::Left => {
                    self.cycle_choice(false);
                    return ModalOutcome::None;
                }
                _ => {}
            }
        }

        let ring = self.ring();
        if let Some(mut position) = ring.iter().position(|f| *f == self.focus)
            && cycle_focus(&mut position, ring.len(), key)
        {
            self.focus = ring[position];
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
            (Self::FOCUS_EDIT_FILE, KeyCode::Enter) => match &self.target {
                Target::Repository { key, .. } => ModalOutcome::Effect(
                    ModalEffect::EditRepositoryFile(key.clone(), ConfigFile::Settings),
                ),
                Target::Global => ModalOutcome::None,
            },
            (_, KeyCode::Enter) => self.save(),
            _ => ModalOutcome::None,
        }
    }
}

fn to_json(settings: &StartWorkSettings) -> Value {
    serde_json::to_value(settings).unwrap_or(Value::Null)
}

/// `field`'s value in a `start_work` section, as the dialog shows it.
/// `None` for a key that is not there, is `null`, or has the wrong shape.
fn lookup(section: &Value, field: &Field) -> Option<String> {
    let value = field
        .path
        .iter()
        .try_fold(section, |value, key| value.get(key))?;
    match (field.kind, value) {
        (Kind::Text | Kind::Scope, Value::String(s)) => Some(s.clone()),
        (Kind::List, Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        (Kind::Count, Value::Number(n)) => Some(n.to_string()),
        (Kind::Toggle, Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn choice_index(kind: Kind, value: &str) -> Option<usize> {
    kind.choices().iter().position(|(v, _)| *v == value)
}

/// A field's value as JSON. `None` for a count that is not one.
fn parse(kind: Kind, value: &str) -> Option<Value> {
    Some(match kind {
        Kind::Text | Kind::Scope => Value::String(value.to_string()),
        Kind::List => Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| Value::String(item.to_string()))
                .collect(),
        ),
        Kind::Count => Value::from(value.trim().parse::<u64>().ok()?),
        Kind::Toggle => Value::Bool(value == "true"),
    })
}
