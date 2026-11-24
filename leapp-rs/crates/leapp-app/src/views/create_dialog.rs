//! The IAM User form: one dialog to create a session or to edit one (spec 14.5, IAM User part).

use leapp_core::regions::{DEFAULT_REGION, REGIONS};
use leapp_core::{Error, Field, IamUserUpdate, NewIamUser, SessionId, SessionKind};
use rune::{El, TextEdit, Tx, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxEvent, ComboboxOption, ComboboxState, Dialog,
    IconButton, IconButtonVariant, Kit, Placement, TextInput, glyphs,
};

use crate::{Msg, Ui};

const DIALOG_WIDTH: f32 = 520.0;
const FIELD_WIDTH: f32 = 340.0;
const LABEL_WIDTH: f32 = 150.0;

const DEFAULT_PROFILE: &str = "default";

/// The placeholder of the key fields in edit mode: an empty key field keeps the saved key.
const KEEP_KEY: &str = "Leave empty to keep the current key";

/// What Submit does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormMode {
    /// Create a new IAM User session.
    Create,
    /// Save new values for the session with this ID.
    Edit(SessionId),
}

/// The fields of the dialog, and whether it is open.
pub struct CreateForm {
    open: bool,
    mode: FormMode,
    name: TextEdit,
    access_key_id: TextEdit,
    /// Secure: the field masks the value, and `TextEdit` wipes its buffers.
    secret_access_key: TextEdit,
    /// True when the eye shows the secret key. Display only: the field stays secure.
    secret_shown: bool,
    profile: TextEdit,
    region: String,
    /// The region list: open or closed, and its search text.
    region_combo: ComboboxState,
    mfa_device: TextEdit,
    errors: FieldErrors,
    /// True while core creates or saves the session, off the render thread.
    saving: bool,
}

/// The keys are left out: they are secrets.
impl std::fmt::Debug for CreateForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateForm")
            .field("open", &self.open)
            .field("mode", &self.mode)
            .field("name", &self.name.text)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field("profile", &self.profile.text)
            .field("region", &self.region)
            .field("mfa_device", &self.mfa_device.text)
            .field("errors", &self.errors)
            .field("saving", &self.saving)
            .finish()
    }
}

/// The text under each field. One slot for each `Field` of core, and one for the form.
#[derive(Debug, Default)]
struct FieldErrors {
    name: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    profile: Option<String>,
    region: Option<String>,
    mfa_device: Option<String>,
    /// An error that is not about one field (for example, the keychain failed).
    form: Option<String>,
}

impl FieldErrors {
    /// Puts `message` under the field that core named.
    fn set(&mut self, field: Field, message: String) {
        *self.slot(field) = Some(message);
    }

    fn slot(&mut self, field: Field) -> &mut Option<String> {
        match field {
            Field::Name => &mut self.name,
            Field::AccessKeyId => &mut self.access_key,
            Field::SecretAccessKey => &mut self.secret_key,
            Field::Profile => &mut self.profile,
            Field::Region => &mut self.region,
            Field::MfaDevice => &mut self.mfa_device,
        }
    }
}

impl Default for CreateForm {
    fn default() -> Self {
        CreateForm {
            open: false,
            mode: FormMode::Create,
            name: TextEdit::default(),
            access_key_id: TextEdit::default(),
            secret_access_key: TextEdit::default().secure(),
            secret_shown: false,
            profile: TextEdit::new(DEFAULT_PROFILE),
            region: DEFAULT_REGION.to_string(),
            region_combo: ComboboxState::default(),
            mfa_device: TextEdit::default(),
            errors: FieldErrors::default(),
            saving: false,
        }
    }
}

impl CreateForm {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn mode(&self) -> FormMode {
        self.mode
    }

    /// True while core creates or saves the session. Submit is disabled.
    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn name(&self) -> &str {
        &self.name.text
    }

    pub fn access_key_id(&self) -> &str {
        &self.access_key_id.text
    }

    pub fn secret_key_is_empty(&self) -> bool {
        self.secret_access_key.text.is_empty()
    }

    /// True when the eye shows the secret key in plain text.
    pub fn secret_shown(&self) -> bool {
        self.secret_shown
    }

    pub fn profile(&self) -> &str {
        &self.profile.text
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn mfa_device(&self) -> &str {
        &self.mfa_device.text
    }

    /// True if name, access key and secret key are empty.
    pub fn is_blank(&self) -> bool {
        self.name.text.is_empty()
            && self.access_key_id.text.is_empty()
            && self.secret_access_key.text.is_empty()
    }

    /// Closes the dialog and sets every field back to its first value. The secret is wiped.
    fn reset(&mut self) {
        *self = CreateForm::default();
    }

    fn new_iam_user(&self) -> NewIamUser {
        NewIamUser {
            name: self.name.text.clone(),
            access_key_id: self.access_key_id.text.clone(),
            secret_access_key: self.secret_access_key.text.clone(),
            profile_name: self.profile.text.clone(),
            region: self.region.clone(),
            mfa_device: Some(self.mfa_device.text.clone()),
        }
    }

    /// Empty key fields keep the saved keys; an empty MFA device removes it (core trims).
    fn update(&self) -> IamUserUpdate {
        IamUserUpdate {
            name: self.name.text.clone(),
            profile_name: self.profile.text.clone(),
            region: self.region.clone(),
            mfa_device: Some(self.mfa_device.text.clone()),
            access_key_id: Some(self.access_key_id.text.clone()),
            secret_access_key: Some(self.secret_access_key.text.clone()),
        }
    }
}

/// Sends the form to core, off the render thread: an edit can stop and start the session, and
/// the start can ask a question that this thread must show. [`done`] gets the result.
fn submit(s: &mut Ui, _tx: &mut Tx) {
    if !s.create.open || s.create.saving {
        return;
    }
    s.create.saving = true;
    let leapp = s.leapp.clone();
    let bridge = s.bridge.clone();
    let send = move |result| bridge.send(Msg::FormDone(result));
    match s.create.mode {
        FormMode::Create => {
            let req = s.create.new_iam_user();
            // `create_iam_user` does blocking keychain and file I/O.
            s.rt.spawn_blocking(move || send(leapp.create_iam_user(req).map(drop)));
        }
        FormMode::Edit(id) => {
            let req = s.create.update();
            s.rt.spawn(async move { send(leapp.update_iam_user(id, req).await) });
        }
    }
}

/// The result of [`submit`]. Success closes the dialog. An error shows under its field, or
/// under the form if it is not about one field; the dialog stays open.
pub(crate) fn done(s: &mut Ui, result: leapp_core::Result<()>) {
    s.create.saving = false;
    match result {
        Ok(()) => s.create.reset(),
        Err(e) => s.create.errors = errors_from(&e),
    }
}

fn errors_from(e: &Error) -> FieldErrors {
    let mut errors = FieldErrors::default();
    match e {
        Error::InvalidInput { field, message } => errors.set(*field, message.clone()),
        other => errors.form = Some(other.to_string()),
    }
    errors
}

/// Opens the empty form in create mode. Ignored while a save runs.
pub(crate) fn open(s: &mut Ui) {
    if s.create.saving {
        return;
    }
    s.create.reset();
    s.create.open = true;
}

/// Opens the form in edit mode with the values of session `id`. The key fields stay empty: an
/// empty key field keeps the saved key. Ignored while a save runs, or if the session is gone.
pub(crate) fn open_edit(s: &mut Ui, id: SessionId) {
    if s.create.saving {
        return;
    }
    let Some(session) = s.ws.session(id) else {
        return;
    };
    let profile = s.ws.profile_name(session.profile_id).unwrap_or_default();
    let SessionKind::IamUser { mfa_device, .. } = &session.kind;
    s.create = CreateForm {
        open: true,
        mode: FormMode::Edit(id),
        name: TextEdit::new(session.name.as_str()),
        profile: TextEdit::new(profile),
        region: session.region.clone(),
        mfa_device: TextEdit::new(mfa_device.as_deref().unwrap_or_default()),
        ..CreateForm::default()
    };
}

/// Opens the dialog with the region list open too. For the headless scene only.
pub(crate) fn open_with_region_list(s: &mut Ui) {
    open(s);
    let current = Some(s.create.region.as_str());
    s.create
        .region_combo
        .step(ComboboxEvent::Toggle { current });
}

/// Closes the form and wipes it. Ignored while a save runs: its result goes to this form.
fn cancel(s: &mut Ui) {
    if s.create.saving {
        return;
    }
    s.create.reset();
}

fn toggle_secret(s: &mut Ui) {
    s.create.secret_shown = !s.create.secret_shown;
}

/// Adds the dialog to `layer` when it is open.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let f = &s.create;
    if !f.open {
        return;
    }
    let editing = matches!(f.mode, FormMode::Edit(_));
    let key_placeholder = |create: &'static str| if editing { KEEP_KEY } else { create };
    let regions: Vec<ComboboxOption> = REGIONS
        .iter()
        .map(|r| ComboboxOption::new(*r, *r))
        .collect();
    let region = Combobox::new("sel-region", &f.region_combo, &regions, |s: &mut Ui| {
        &mut s.create.region_combo
    })
    .current(Some(&f.region))
    .width(FIELD_WIDTH)
    .placement(Placement::BottomStart)
    .label("Region")
    .search_placeholder("Search regions")
    .empty_message("No region matches")
    .on_choose(|s: &mut Ui, region: &str, _tx: &mut Tx| {
        s.create.region = region.to_string();
        s.create.errors.region = None;
    })
    .build(k, layer);

    let eye = IconButton::new(if f.secret_shown {
        &glyphs::EYE_OFF
    } else {
        &glyphs::EYE
    })
    .variant(IconButtonVariant::Ghost)
    .label(if f.secret_shown {
        "Hide secret key"
    } else {
        "Show secret key"
    })
    .on_press(toggle_secret)
    .build(k)
    .key("btn-secret-eye");

    let body = col()
        .key("dlg-iam-user")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(field(
            k,
            "Session Alias",
            "err-name",
            f.errors.name.as_deref(),
            TextInput::new(&f.name)
                .placeholder("Session Alias")
                .label("Session Alias")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.name.is_some())
                .on_edit(|s: &mut Ui| &mut s.create.name)
                .on_change(|s: &mut Ui, _tx: &mut Tx| s.create.errors.name = None)
                .build(k)
                .key("fld-name"),
        ))
        .child(field(
            k,
            "AWS Access Key ID",
            "err-access-key",
            f.errors.access_key.as_deref(),
            TextInput::new(&f.access_key_id)
                .placeholder(key_placeholder("AWS Access Key ID"))
                .label("AWS Access Key ID")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.access_key.is_some())
                .on_edit(|s: &mut Ui| &mut s.create.access_key_id)
                .on_change(|s: &mut Ui, _tx: &mut Tx| s.create.errors.access_key = None)
                .build(k)
                .key("fld-access-key"),
        ))
        .child(field(
            k,
            "AWS Secret Access Key",
            "err-secret-key",
            f.errors.secret_key.as_deref(),
            // Secure even when shown: the eye changes only what the field draws.
            TextInput::new(&f.secret_access_key)
                .secure()
                .revealed(f.secret_shown)
                .placeholder(key_placeholder("AWS Secret Access Key"))
                .label("AWS Secret Access Key")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.secret_key.is_some())
                .trailing(eye)
                .on_edit(|s: &mut Ui| &mut s.create.secret_access_key)
                .on_change(|s: &mut Ui, _tx: &mut Tx| s.create.errors.secret_key = None)
                .build(k)
                .key("fld-secret-key"),
        ))
        .child(field(
            k,
            "Named Profile",
            "err-profile",
            f.errors.profile.as_deref(),
            TextInput::new(&f.profile)
                .placeholder("Named Profile")
                .label("Named Profile")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.profile.is_some())
                .on_edit(|s: &mut Ui| &mut s.create.profile)
                .on_change(|s: &mut Ui, _tx: &mut Tx| s.create.errors.profile = None)
                .build(k)
                .key("fld-profile"),
        ))
        .child(field(
            k,
            "Region",
            "err-region",
            f.errors.region.as_deref(),
            el().key("sel-region").child(region),
        ))
        .child(field(
            k,
            "MFA Device",
            "err-mfa-device",
            f.errors.mfa_device.as_deref(),
            TextInput::new(&f.mfa_device)
                .placeholder("MFA device ARN (optional)")
                .label("MFA Device")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.mfa_device.is_some())
                .on_edit(|s: &mut Ui| &mut s.create.mfa_device)
                .on_change(|s: &mut Ui, _tx: &mut Tx| s.create.errors.mfa_device = None)
                .build(k)
                .key("fld-mfa-device"),
        ))
        .children(f.errors.form.as_deref().map(|m| {
            text(m.to_string())
                .key("err-form")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg)
        }))
        .child(buttons(k, editing, f.saving));

    let title = if editing {
        "Edit IAM User"
    } else {
        "Create IAM User"
    };
    Dialog::new("dlg-create", title)
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(cancel)
        .child(body)
        .build(k, layer);
}

/// A label on the left, the input on the right, and the error text under the input.
fn field(k: &Kit, label: &str, err_key: &str, err: Option<&str>, input: El) -> El {
    let label = text(label.to_string())
        .font(k.ty.ui)
        .color(k.c.text_base)
        .w(LABEL_WIDTH);
    let mut right = col().gap(4.0).child(input);
    if let Some(message) = err {
        right = right.child(
            text(message.to_string())
                .key(err_key)
                .role("alert")
                .font(k.ty.small)
                .color(k.c.danger.fg),
        );
    }
    row().w_full().gap(12.0).child(label).child(right)
}

/// Cancel and Submit. Both are disabled while a save runs.
fn buttons(k: &Kit, editing: bool, saving: bool) -> El {
    let submit_label = if editing { "Save" } else { "Create Session" };
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
                .key("btn-create-cancel"),
        )
        .child(
            Button::new(submit_label)
                .disabled(saving)
                .on_press_tx(submit)
                .build(k)
                .key("btn-create-submit"),
        )
}
