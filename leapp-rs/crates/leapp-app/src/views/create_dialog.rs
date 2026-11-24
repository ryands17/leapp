//! The Create IAM User dialog (spec 14.5, IAM User part, without MFA).

use leapp_core::regions::{DEFAULT_REGION, REGIONS};
use leapp_core::{Error, Field, NewIamUser};
use rune::{El, TextEdit, Tx, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Combobox, ComboboxEvent, ComboboxOption, ComboboxState, Dialog, Kit,
    Placement, TextInput,
};

use crate::Ui;

const DIALOG_WIDTH: f32 = 520.0;
const FIELD_WIDTH: f32 = 340.0;
const LABEL_WIDTH: f32 = 150.0;

const DEFAULT_PROFILE: &str = "default";

/// The fields of the dialog, and whether it is open.
#[derive(Debug)]
pub struct CreateForm {
    open: bool,
    name: TextEdit,
    access_key_id: TextEdit,
    /// Secure: the field masks the value, and `TextEdit` wipes its buffers.
    secret_access_key: TextEdit,
    profile: TextEdit,
    region: String,
    /// The region list: open or closed, and its search text.
    region_combo: ComboboxState,
    errors: FieldErrors,
}

/// The text under each field. One slot for each `Error::InvalidInput` field name of core.
#[derive(Debug, Default)]
struct FieldErrors {
    name: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    profile: Option<String>,
    region: Option<String>,
    /// An error that is not about one field (for example, the keychain failed).
    form: Option<String>,
}

impl FieldErrors {
    /// Puts `message` under the field that core named. An unknown name goes to the form.
    fn set(&mut self, field: Field, message: String) {
        let slot = match field {
            Field::Name => &mut self.name,
            Field::AccessKeyId => &mut self.access_key,
            Field::SecretAccessKey => &mut self.secret_key,
            Field::Profile => &mut self.profile,
            Field::Region => &mut self.region,
            // Task 7 adds the MFA device field.
            Field::MfaDevice => &mut self.form,
        };
        *slot = Some(message);
    }
}

impl Default for CreateForm {
    fn default() -> Self {
        CreateForm {
            open: false,
            name: TextEdit::default(),
            access_key_id: TextEdit::default(),
            secret_access_key: TextEdit::default().secure(),
            profile: TextEdit::new(DEFAULT_PROFILE),
            region: DEFAULT_REGION.to_string(),
            region_combo: ComboboxState::default(),
            errors: FieldErrors::default(),
        }
    }
}

impl CreateForm {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn profile(&self) -> &str {
        &self.profile.text
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// True if name, access key and secret key are empty.
    pub fn is_blank(&self) -> bool {
        self.name.text.is_empty()
            && self.access_key_id.text.is_empty()
            && self.secret_access_key.text.is_empty()
    }

    fn open(&mut self) {
        self.open = true;
    }

    /// Closes the dialog and sets every field back to its first value. The secret is wiped.
    fn reset(&mut self) {
        *self = CreateForm::default();
    }

    fn request(&self) -> NewIamUser {
        NewIamUser {
            name: self.name.text.clone(),
            access_key_id: self.access_key_id.text.clone(),
            secret_access_key: self.secret_access_key.text.clone(),
            profile_name: self.profile.text.clone(),
            region: self.region.clone(),
            mfa_device: None,
        }
    }
}

/// Creates the session. On success the dialog closes and the fields clear; on a validation
/// error the message shows under its field and the dialog stays open.
fn submit(s: &mut Ui, _tx: &mut Tx) {
    match s.leapp.create_iam_user(s.create.request()) {
        Ok(_) => {
            s.create.reset();
            s.refresh();
        }
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

pub(crate) fn open(s: &mut Ui) {
    s.create.open();
}

/// Opens the dialog with the region list open too. For the headless scene only.
pub(crate) fn open_with_region_list(s: &mut Ui) {
    s.create.open();
    let current = Some(s.create.region.as_str());
    s.create
        .region_combo
        .step(ComboboxEvent::Toggle { current });
}

fn cancel(s: &mut Ui) {
    s.create.reset();
}

/// Adds the dialog to `layer` when it is open.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let f = &s.create;
    if !f.open {
        return;
    }
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
                .build(k)
                .key("fld-name"),
        ))
        .child(field(
            k,
            "AWS Access Key ID",
            "err-access-key",
            f.errors.access_key.as_deref(),
            TextInput::new(&f.access_key_id)
                .placeholder("AWS Access Key ID")
                .label("AWS Access Key ID")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.access_key.is_some())
                .on_edit(|s: &mut Ui| &mut s.create.access_key_id)
                .build(k)
                .key("fld-access-key"),
        ))
        .child(field(
            k,
            "AWS Secret Access Key",
            "err-secret-key",
            f.errors.secret_key.as_deref(),
            TextInput::new(&f.secret_access_key)
                .secure()
                .placeholder("AWS Secret Access Key")
                .label("AWS Secret Access Key")
                .width(Some(FIELD_WIDTH))
                .invalid(f.errors.secret_key.is_some())
                .on_edit(|s: &mut Ui| &mut s.create.secret_access_key)
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
        .children(f.errors.form.as_deref().map(|m| {
            text(m.to_string())
                .key("err-form")
                .font(k.ty.small)
                .color(k.c.danger.fg)
        }))
        .child(buttons(k));

    Dialog::new("dlg-create", "Create IAM User")
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

fn buttons(k: &Kit) -> El {
    row()
        .w_full()
        .pt(8.0)
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .on_press(cancel)
                .build(k)
                .key("btn-create-cancel"),
        )
        .child(
            Button::new("Create Session")
                .on_press_tx(submit)
                .build(k)
                .key("btn-create-submit"),
        )
}
