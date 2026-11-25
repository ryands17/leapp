//! The IAM Role Chained form (spec 14.5, IAM Role Chained part): create mode. The parent list
//! ([`parent_options`]) also serves the edit mode, which leaves out the edited session and the
//! sessions below it.

use std::collections::HashSet;

use leapp_core::{
    DEFAULT_ROLE_SESSION_NAME, Error, Field, NewIamRoleChained, SessionId, Workspace,
};
use rune::{Color, El, TextEdit, Tx, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxOption, ComboboxState, Dialog, Kit, Placement,
    TextInput,
};

use super::fields::{FIELD_WIDTH, ProfileChoice, RegionChoice, field, profile_field, region_field};
use crate::{Msg, Ui};

const DIALOG_WIDTH: f32 = 520.0;

const DEFAULT_PROFILE: &str = "default";

/// Leapp's IAM Role Chained chip colour: `$primary-color` (`#9046ff`) of the Leapp SCSS, which
/// its `badge-purple` uses.
pub(crate) fn purple() -> Color {
    let c = |v: u8| rune::color::srgb_decode(f32::from(v) / 255.0);
    Color {
        r: c(0x90),
        g: c(0x46),
        b: c(0xff),
        a: 1.0,
    }
}

/// The fields of the dialog, and whether it is open. It holds no secrets.
#[derive(Default)]
pub struct ChainedForm {
    open: bool,
    name: TextEdit,
    /// The chosen parent ("Assumer session"). `None` until the user chooses one.
    parent: Option<SessionId>,
    parent_combo: ComboboxState,
    role_arn: TextEdit,
    role_session_name: TextEdit,
    external_id: TextEdit,
    profile: ProfileChoice,
    region: RegionChoice,
    errors: FieldErrors,
    /// The token of the save that runs now. Only the result with this token goes to the form.
    saving: Option<u64>,
    /// The token of the next save.
    next_token: u64,
}

impl std::fmt::Debug for ChainedForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainedForm")
            .field("open", &self.open)
            .field("name", &self.name.text)
            .field("parent", &self.parent)
            .field("role_arn", &self.role_arn.text)
            .field("role_session_name", &self.role_session_name.text)
            .field("external_id", &self.external_id.text)
            .field("profile", &self.profile.name)
            .field("region", &self.region.region)
            .field("errors", &self.errors)
            .field("saving", &self.saving)
            .finish()
    }
}

/// The text under each field. One slot for each field of the form, and one for the form.
#[derive(Debug, Default)]
struct FieldErrors {
    name: Option<String>,
    parent: Option<String>,
    role_arn: Option<String>,
    role_session_name: Option<String>,
    external_id: Option<String>,
    profile: Option<String>,
    region: Option<String>,
    /// An error that is not about one field of this form.
    form: Option<String>,
}

impl FieldErrors {
    fn from_error(e: &Error) -> FieldErrors {
        let mut errors = FieldErrors::default();
        let message = match e {
            Error::InvalidInput { message, .. } => message.clone(),
            other => other.to_string(),
        };
        let slot = match e {
            Error::InvalidInput { field, .. } => match field {
                Field::Name => &mut errors.name,
                Field::ParentSession => &mut errors.parent,
                Field::RoleArn => &mut errors.role_arn,
                Field::RoleSessionName => &mut errors.role_session_name,
                Field::ExternalId => &mut errors.external_id,
                Field::Profile => &mut errors.profile,
                Field::Region => &mut errors.region,
                // The IAM User fields are not in this form.
                Field::AccessKeyId | Field::SecretAccessKey | Field::MfaDevice => &mut errors.form,
            },
            _ => &mut errors.form,
        };
        *slot = Some(message);
        errors
    }
}

impl ChainedForm {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// True while core creates the session. Submit and Cancel are disabled.
    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// True when the required fields (name, parent, role ARN) are filled and no save runs.
    pub fn can_submit(&self) -> bool {
        self.saving.is_none()
            && !self.name.text.trim().is_empty()
            && self.parent.is_some()
            && !self.role_arn.text.trim().is_empty()
    }

    /// Closes the dialog and sets every field back to its first value. The token count stays,
    /// so a late result of an old save never matches a new one.
    fn reset(&mut self) {
        let next_token = self.next_token;
        *self = ChainedForm {
            next_token,
            ..ChainedForm::default()
        };
    }

    /// The request of the form. `None` if no parent is chosen.
    fn request(&self) -> Option<NewIamRoleChained> {
        Some(NewIamRoleChained {
            name: self.name.text.clone(),
            parent_session_id: self.parent?,
            role_arn: self.role_arn.text.clone(),
            // Core trims: an empty field means not set.
            role_session_name: Some(self.role_session_name.text.clone()),
            external_id: Some(self.external_id.text.clone()),
            profile_name: self.profile.name.clone(),
            region: self.region.region.clone(),
        })
    }
}

/// The options of the parent list: every session, by name, in workspace order. `edited` is the
/// session of an edit: it and the sessions below it are left out (a parent there makes a loop).
pub(crate) fn parent_options(ws: &Workspace, edited: Option<SessionId>) -> Vec<ComboboxOption> {
    let excluded: HashSet<SessionId> = match edited {
        Some(id) => ws.descendants(id).into_iter().chain([id]).collect(),
        None => HashSet::new(),
    };
    ws.sessions
        .iter()
        .filter(|s| !excluded.contains(&s.id))
        .map(|s| ComboboxOption::new(s.id.to_string(), s.name.as_str()))
        .collect()
}

/// Opens the empty form, on the `default` profile and the default region of the settings.
/// Ignored while a save runs.
pub(crate) fn open(s: &mut Ui) {
    let f = &mut s.chained;
    if f.saving.is_some() {
        return;
    }
    f.reset();
    f.profile = ProfileChoice::new(DEFAULT_PROFILE);
    f.region = RegionChoice::new(s.ws.settings.default_region.as_str());
    f.open = true;
}

/// Closes the form and wipes it. Ignored while a save runs: its result goes to this form.
fn cancel(s: &mut Ui) {
    if s.chained.saving.is_some() {
        return;
    }
    s.chained.reset();
}

/// Sends the form to core, off the render thread (`create_iam_role_chained` does blocking file
/// I/O). [`done`] gets the result. Ignored unless [`ChainedForm::can_submit`].
fn submit(s: &mut Ui, _tx: &mut Tx) {
    let f = &mut s.chained;
    if !f.open || !f.can_submit() {
        return;
    }
    let Some(req) = f.request() else {
        return;
    };
    let token = f.next_token;
    f.next_token = token.wrapping_add(1);
    f.saving = Some(token);
    let leapp = s.leapp.clone();
    let bridge = s.bridge.clone();
    let task =
        s.rt.spawn_blocking(move || leapp.create_iam_role_chained(req).map(drop));
    // Always send a result, also if the task panics: else the form stays locked.
    s.rt.spawn(async move {
        let result = task.await.unwrap_or_else(|_| {
            Err(Error::Io(std::io::Error::other(
                "the save stopped unexpectedly; try again",
            )))
        });
        bridge.send(Msg::ChainedDone { token, result });
    });
}

/// The result of the save with `token`. Success closes the form. An error shows under its field,
/// or under the form if it is not about one field; the form stays open. A result of another
/// save only shows its error in the toast.
pub(crate) fn done(s: &mut Ui, token: u64, result: leapp_core::Result<()>) {
    let f = &mut s.chained;
    if f.saving != Some(token) {
        if let Err(e) = result {
            s.show_error(e.to_string());
        }
        return;
    }
    f.saving = None;
    match result {
        Ok(()) => f.reset(),
        Err(e) => f.errors = FieldErrors::from_error(&e),
    }
}

/// A text field of the form, keyed `key`, with its error `err` under it.
#[allow(clippy::too_many_arguments)]
fn text_field(
    k: &Kit,
    label: &str,
    placeholder: &str,
    key: &str,
    err_key: &str,
    err: Option<&str>,
    value: &TextEdit,
    on: fn(&mut Ui) -> &mut TextEdit,
    clear: fn(&mut FieldErrors),
) -> El {
    field(
        k,
        label,
        err_key,
        err,
        TextInput::new(value)
            .placeholder(placeholder)
            .label(label)
            .width(Some(FIELD_WIDTH))
            .invalid(err.is_some())
            .on_edit(on)
            .on_change(move |s: &mut Ui, _tx: &mut Tx| clear(&mut s.chained.errors))
            .build(k)
            .key(key),
    )
}

/// Adds the dialog to `layer` when it is open.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let f = &s.chained;
    if !f.open {
        return;
    }
    let e = &f.errors;
    let parent_key = "sel-parent";
    let current = f.parent.map(|id| id.to_string());
    let parent = Combobox::new(
        parent_key,
        &f.parent_combo,
        &parent_options(&s.ws, None),
        |s: &mut Ui| &mut s.chained.parent_combo,
    )
    .current(current.as_deref())
    .width(FIELD_WIDTH)
    .placement(Placement::BottomStart)
    .label("Assumer session")
    .placeholder("Choose a session")
    .search_placeholder("Search sessions")
    .empty_message("No session matches")
    .on_choose(|s: &mut Ui, id: &str, _tx: &mut Tx| {
        if let Ok(id) = id.parse::<SessionId>() {
            s.chained.parent = Some(id);
            s.chained.errors.parent = None;
        }
    })
    .build(k, layer);
    let parent = el().key(parent_key).child(parent);
    let profile = profile_field(
        "sel-profile",
        &f.profile,
        &s.ws.profiles,
        |s: &mut Ui| &mut s.chained.profile,
        |s: &mut Ui| s.chained.errors.profile = None,
        k,
        layer,
    );
    let region = region_field(
        "sel-region",
        &f.region,
        |s: &mut Ui| &mut s.chained.region,
        |s: &mut Ui| s.chained.errors.region = None,
        k,
        layer,
    );

    let body = col()
        .key("dlg-iam-role-chained")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(text_field(
            k,
            "Session alias",
            "Session alias",
            "fld-chained-name",
            "err-chained-name",
            e.name.as_deref(),
            &f.name,
            |s| &mut s.chained.name,
            |e| e.name = None,
        ))
        .child(field(
            k,
            "Assumer session",
            "err-parent",
            e.parent.as_deref(),
            parent,
        ))
        .child(text_field(
            k,
            "Role ARN",
            "arn:aws:iam::123456789012:role/name",
            "fld-role-arn",
            "err-role-arn",
            e.role_arn.as_deref(),
            &f.role_arn,
            |s| &mut s.chained.role_arn,
            |e| e.role_arn = None,
        ))
        .child(text_field(
            k,
            "Role session name",
            DEFAULT_ROLE_SESSION_NAME,
            "fld-role-session-name",
            "err-role-session-name",
            e.role_session_name.as_deref(),
            &f.role_session_name,
            |s| &mut s.chained.role_session_name,
            |e| e.role_session_name = None,
        ))
        .child(text_field(
            k,
            "External ID",
            "External ID (optional)",
            "fld-external-id",
            "err-external-id",
            e.external_id.as_deref(),
            &f.external_id,
            |s| &mut s.chained.external_id,
            |e| e.external_id = None,
        ))
        .child(field(
            k,
            "Named profile",
            "err-profile",
            e.profile.as_deref(),
            profile,
        ))
        .child(field(
            k,
            "Region",
            "err-region",
            e.region.as_deref(),
            region,
        ))
        .children(e.form.as_deref().map(|m| {
            text(m.to_string())
                .key("err-chained-form")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg)
        }))
        .child(buttons(k, f.can_submit(), f.saving.is_some()));

    // The rune-kit `Dialog` keys its panel `<key>-dialog`.
    Dialog::new("dlg-chained", "Create IAM Role Chained")
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(cancel)
        .child(body)
        .build(k, layer);
}

/// Cancel and Create Session. Cancel is disabled while a save runs; Create Session is disabled
/// until the required fields are filled, and while a save runs.
fn buttons(k: &Kit, can_submit: bool, saving: bool) -> El {
    row()
        .w_full()
        .pt(8.0)
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .disabled(saving)
                .on_press(cancel)
                .build(k)
                .key("btn-chained-cancel"),
        )
        .child(
            Button::new("Create Session")
                .disabled(!can_submit)
                .on_press_tx(submit)
                .build(k)
                .key("btn-chained-submit"),
        )
}
