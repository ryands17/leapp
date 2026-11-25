//! The Settings dialog (spec 14.8, tabs like Leapp `options-dialog`): the General tab with the
//! default region, and the Profiles tab with the named profiles (F23-R13).

use leapp_core::{Error, NamedProfile, Settings};
use rune::{El, Icon, ScrollSpec, TextEdit, Tx, col, el, hairline, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, IconButton, IconButtonVariant, Kit, TextInput};
use uuid::Uuid;

use super::fields::{FIELD_WIDTH, RegionChoice, field, region_field};
use crate::{Msg, Ui};

const DIALOG_WIDTH: f32 = 560.0;
const CONFIRM_WIDTH: f32 = 400.0;
/// The profile list scrolls when it is taller than this (Leapp `max-height-230`).
const LIST_MAX_HEIGHT: f32 = 230.0;
const PROFILE_ROW_HEIGHT: f32 = 40.0;
/// Its own name: Leapp shows no rename or delete for it (F23-R2).
const DEFAULT_PROFILE: &str = "default";

/// The gear of the toolbar.
pub(crate) static GEAR: Icon = Icon {
    name: "leapp-gear",
    view: 24.0,
    // The Lucide `settings` icon (ISC licence).
    body: r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.5"/>"#,
};

/// Rename (Leapp `moon-Edit`).
static PENCIL: Icon = Icon {
    name: "leapp-pencil",
    view: 16.0,
    body: r#"<path d="M10.5 2.5L13.5 5.5L5.5 13.5H2.5V10.5L10.5 2.5Z" stroke="currentColor" stroke-linejoin="round"/>"#,
};

/// Delete (Leapp `moon-Delete`).
static TRASH: Icon = Icon {
    name: "leapp-trash",
    view: 16.0,
    body: r#"<path d="M2.5 4.5H13.5M6 4.5V2.5H10V4.5M4 4.5L4.8 13.5H11.2L12 4.5" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

/// Add (Leapp `moon-Add`).
static PLUS: Icon = Icon {
    name: "leapp-plus",
    view: 16.0,
    body: r#"<path d="M8 3V13M3 8H13" stroke="currentColor" stroke-linecap="round"/>"#,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    General,
    Profiles,
}

/// The open rename of one profile.
#[derive(Debug)]
pub(crate) struct Rename {
    profile_id: Uuid,
    name: TextEdit,
    error: Option<String>,
    /// True while core renames the profile, off the render thread.
    saving: bool,
}

/// The question "delete this profile?".
#[derive(Debug)]
pub(crate) struct DeleteProfileAsk {
    profile_id: Uuid,
    name: String,
    /// The sessions on the profile when the user asked.
    sessions: usize,
    /// When the confirm was first polled; `None` before that.
    pub(crate) since: Option<f64>,
    /// True [`crate::CONFIRM_ARM_SECONDS`] after `since`: the buttons take answers.
    pub(crate) armed: bool,
}

/// The state of the Settings dialog.
#[derive(Debug, Default)]
pub(crate) struct SettingsState {
    open: bool,
    tab: Tab,
    region: RegionChoice,
    new_profile: TextEdit,
    new_profile_error: Option<String>,
    rename: Option<Rename>,
    pub(crate) delete: Option<DeleteProfileAsk>,
}

/// Opens the dialog on the General tab, with the saved default region.
pub(crate) fn open(s: &mut Ui) {
    s.settings = SettingsState {
        open: true,
        region: RegionChoice::new(s.ws.settings.default_region.as_str()),
        ..SettingsState::default()
    };
}

/// Opens the dialog on the Profiles tab. For the headless scene only.
pub(crate) fn open_profiles(s: &mut Ui) {
    open(s);
    show_tab(s, Tab::Profiles);
}

/// Closes the dialog. A rename that runs still ends; its error shows in the toast.
fn close(s: &mut Ui) {
    s.settings = SettingsState::default();
}

fn show_tab(s: &mut Ui, tab: Tab) {
    s.settings.tab = tab;
}

/// Saves the chosen default region. An error shows in the toast.
fn save_region(s: &mut Ui) {
    let region = s.settings.region.region.clone();
    if region == s.ws.settings.default_region {
        return;
    }
    let settings = Settings {
        default_region: region,
    };
    if let Err(e) = s.leapp.update_settings(settings) {
        s.show_error(e.to_string());
    }
    s.refresh();
}

/// Adds the typed profile. A name error shows under the field; the field is emptied on success.
fn add_profile(s: &mut Ui, _tx: &mut Tx) {
    match s.leapp.create_profile(&s.settings.new_profile.text) {
        Ok(_) => {
            s.settings.new_profile = TextEdit::default();
            s.settings.new_profile_error = None;
        }
        Err(Error::InvalidInput { message, .. }) => s.settings.new_profile_error = Some(message),
        Err(e) => s.show_error(e.to_string()),
    }
    s.refresh();
}

/// Opens the rename field of profile `profile_id`, filled with its name and focused. Ignored
/// while a rename runs.
fn start_rename(s: &mut Ui, profile_id: Uuid) {
    if s.settings.rename.as_ref().is_some_and(|r| r.saving) {
        return;
    }
    let Some(name) = s.ws.profile_name(profile_id) else {
        return;
    };
    let mut name = TextEdit::new(name);
    name.focused = true;
    s.settings.rename = Some(Rename {
        profile_id,
        name,
        error: None,
        saving: false,
    });
}

/// Renames the profile off the render thread: a session on it can stop and start again, and
/// the start can ask a question. [`rename_done`] gets the result.
fn save_rename(s: &mut Ui, _tx: &mut Tx) {
    let Some(rename) = s.settings.rename.as_mut().filter(|r| !r.saving) else {
        return;
    };
    rename.saving = true;
    let profile_id = rename.profile_id;
    let name = rename.name.text.clone();
    let leapp = s.leapp.clone();
    let bridge = s.bridge.clone();
    let task =
        s.rt.spawn(async move { leapp.rename_profile(profile_id, &name).await });
    // Always send a result, also if the task panics: else the field stays locked.
    s.rt.spawn(async move {
        let result = task.await.unwrap_or_else(|_| {
            Err(Error::Io(std::io::Error::other(
                "the rename stopped unexpectedly; try again",
            )))
        });
        bridge.send(Msg::RenameDone { profile_id, result });
    });
}

/// The result of [`save_rename`]. A name error shows under the field, which stays open. Any
/// other error shows in the toast: a failed restart keeps the new name (F23-R4).
pub(crate) fn rename_done(s: &mut Ui, profile_id: Uuid, result: leapp_core::Result<()>) {
    let field = s
        .settings
        .rename
        .as_mut()
        .filter(|r| r.profile_id == profile_id);
    match (result, field) {
        (Err(Error::InvalidInput { message, .. }), Some(rename)) => {
            rename.saving = false;
            rename.error = Some(message);
        }
        (result, _) => {
            if s.settings
                .rename
                .as_ref()
                .is_some_and(|r| r.profile_id == profile_id)
            {
                s.settings.rename = None;
            }
            if let Err(e) = result {
                s.show_error(e.to_string());
            }
        }
    }
}

/// Asks the user if profile `profile_id` can be deleted.
fn ask_delete(s: &mut Ui, profile_id: Uuid) {
    let Some(name) = s.ws.profile_name(profile_id) else {
        return;
    };
    s.settings.delete = Some(DeleteProfileAsk {
        profile_id,
        name: name.to_string(),
        sessions: s.ws.sessions_on(profile_id),
        since: None,
        armed: false,
    });
}

/// Answers the delete confirm. Yes deletes the profile off the render thread (its sessions can
/// restart); an error shows in the toast. Ignored until the confirm is armed.
fn answer_delete(s: &mut Ui, yes: bool) {
    if !s.settings.delete.as_ref().is_some_and(|d| d.armed) {
        return;
    }
    let Some(ask) = s.settings.delete.take() else {
        return;
    };
    if yes {
        if s.settings
            .rename
            .as_ref()
            .is_some_and(|r| r.profile_id == ask.profile_id && !r.saving)
        {
            s.settings.rename = None;
        }
        let leapp = s.leapp.clone();
        s.run(async move { leapp.delete_profile(ask.profile_id).await });
    }
}

/// The question of the delete confirm (F23-R5).
fn delete_question(name: &str, sessions: usize) -> String {
    match sessions {
        0 => format!("Delete profile {name}?"),
        1 => format!(
            "Delete profile {name}? 1 session that uses it will move to the default profile."
        ),
        n => format!(
            "Delete profile {name}? {n} sessions that use it will move to the default profile."
        ),
    }
}

/// Adds the dialog, and its delete confirm, to `layer` when it is open.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let st = &s.settings;
    if !st.open {
        return;
    }
    let content = match st.tab {
        Tab::General => general(st, k, layer),
        Tab::Profiles => profiles(s, k),
    };
    let body = col()
        .key("dlg-settings")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(tabs(st.tab, k))
        .child(hairline(k.c.border_muted))
        .child(content)
        .child(
            row().w_full().pt(8.0).justify_end().child(
                Button::new("Close")
                    .on_press(close)
                    .build(k)
                    .key("btn-settings-close"),
            ),
        );
    Dialog::new("dlg-set", "Settings")
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(close)
        .child(body)
        .build(k, layer);
    if let Some(ask) = &st.delete {
        delete_confirm(ask, k, layer);
    }
}

/// The tab bar. rune-kit `SegmentedControl` keys its segments `segment<i>` and takes no key for
/// them, so the tabs are two buttons.
fn tabs(current: Tab, k: &Kit) -> El {
    let tab = |label: &str, key: &str, this: Tab| {
        Button::new(label)
            .variant(if current == this {
                ButtonVariant::Neutral
            } else {
                ButtonVariant::GhostMuted
            })
            .on_press(move |s: &mut Ui| show_tab(s, this))
            .build(k)
            .key(key)
            .role("tab")
    };
    row()
        .role("tablist")
        .gap(4.0)
        .child(tab("General", "tab-general", Tab::General))
        .child(tab("Profiles", "tab-profiles", Tab::Profiles))
}

fn heading(label: &str, k: &Kit) -> El {
    text(label.to_string())
        .font(k.ty.ui_medium)
        .color(k.c.text_base)
}

fn general(st: &SettingsState, k: &Kit, layer: &mut Vec<El>) -> El {
    let region = region_field(
        "sel-default-region",
        &st.region,
        |s: &mut Ui| &mut s.settings.region,
        save_region,
        k,
        layer,
    );
    col()
        .w_full()
        .gap(12.0)
        .child(heading("Default Regions", k))
        .child(field(
            k,
            "Default region",
            "err-default-region",
            None,
            region,
        ))
}

fn profiles(s: &Ui, k: &Kit) -> El {
    let st = &s.settings;
    let input = TextInput::new(&st.new_profile)
        .placeholder("Add a new Named Profile")
        .label("New named profile")
        .width(Some(FIELD_WIDTH))
        .invalid(st.new_profile_error.is_some())
        .on_edit(|s: &mut Ui| &mut s.settings.new_profile)
        .on_change(|s: &mut Ui, _tx: &mut Tx| s.settings.new_profile_error = None)
        .on_submit(add_profile)
        .build(k)
        .key("fld-new-profile");
    let add = IconButton::new(&PLUS)
        .label("Add profile")
        .on_press_tx(add_profile)
        .build(k)
        .key("btn-profile-add");
    let mut add_row = col()
        .gap(4.0)
        .child(row().gap(8.0).center_y().child(input).child(add));
    if let Some(message) = &st.new_profile_error {
        add_row = add_row.child(error_text("err-new-profile", message, k));
    }
    let mut list = col().w_full();
    for profile in &s.ws.profiles {
        list = list.child(profile_row(st, profile, k));
    }
    col()
        .w_full()
        .gap(12.0)
        .child(heading("Named Profiles", k))
        .child(add_row)
        .child(list.max_h(LIST_MAX_HEIGHT).scroll(ScrollSpec {
            stick_bottom: false,
        }))
}

fn error_text(key: &str, message: &str, k: &Kit) -> El {
    text(message.to_string())
        .key(key)
        .role("alert")
        .font(k.ty.small)
        .color(k.c.danger.fg)
}

/// One profile: its name, or its rename field; rename and delete buttons unless it is
/// `default`.
fn profile_row(st: &SettingsState, profile: &NamedProfile, k: &Kit) -> El {
    let id = profile.id;
    let renaming = st.rename.as_ref().filter(|r| r.profile_id == id);
    let mut r = row()
        .key(format!("profile-row-{id}"))
        .w_full()
        .min_h(PROFILE_ROW_HEIGHT)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted);
    if let Some(rename) = renaming {
        let input = TextInput::new(&rename.name)
            .label("Profile name")
            .width(Some(FIELD_WIDTH))
            .invalid(rename.error.is_some())
            .disabled(rename.saving)
            .on_edit(|s: &mut Ui| {
                &mut s
                    .settings
                    .rename
                    .as_mut()
                    .expect("the rename field exists only while a rename is open")
                    .name
            })
            .on_change(|s: &mut Ui, _tx: &mut Tx| {
                if let Some(r) = s.settings.rename.as_mut() {
                    r.error = None;
                }
            })
            .on_submit(save_rename)
            .build(k)
            .key(format!("fld-profile-rename-{id}"));
        let mut left = col().grow(1.0).gap(4.0).child(input);
        if let Some(message) = &rename.error {
            left = left.child(error_text(&format!("err-profile-rename-{id}"), message, k));
        }
        r = r.py(4.0).child(left).child(
            Button::new("Save")
                .disabled(rename.saving)
                .on_press_tx(save_rename)
                .build(k)
                .key(format!("btn-profile-rename-save-{id}")),
        );
    } else {
        r = r.child(
            text(profile.name.clone())
                .key(format!("profile-name-{id}"))
                .font(k.ty.ui)
                .color(k.c.text_base)
                .grow(1.0),
        );
        if profile.name != DEFAULT_PROFILE {
            r = r.child(
                IconButton::new(&PENCIL)
                    .variant(IconButtonVariant::Ghost)
                    .label(format!("Rename {}", profile.name))
                    .on_press(move |s: &mut Ui| start_rename(s, id))
                    .build(k)
                    .key(format!("btn-profile-rename-{id}")),
            );
        }
    }
    if profile.name != DEFAULT_PROFILE {
        r = r.child(
            IconButton::new(&TRASH)
                .variant(IconButtonVariant::Ghost)
                .label(format!("Delete {}", profile.name))
                .on_press(move |s: &mut Ui| ask_delete(s, id))
                .build(k)
                .key(format!("btn-profile-delete-{id}")),
        );
    } else {
        // Keeps the rows the same height as the rows with buttons.
        r = r.child(el().h(28.0));
    }
    r
}

/// The armed question "delete this profile?" (F23-R5).
fn delete_confirm(ask: &DeleteProfileAsk, k: &Kit, layer: &mut Vec<El>) {
    let disabled = !ask.armed;
    // The first child holds the question: the tests read it as the text of the dialog.
    let body = col()
        .key("dlg-delete-profile")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(
            text(delete_question(&ask.name, ask.sessions))
                .font(k.ty.ui)
                .color(k.c.text_base)
                .wrap(),
        )
        .child(
            row()
                .w_full()
                .pt(8.0)
                .gap(8.0)
                .justify_end()
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Ghost)
                        .disabled(disabled)
                        .on_press(|s: &mut Ui| answer_delete(s, false))
                        .build(k)
                        .key("btn-delete-profile-no"),
                )
                .child(
                    Button::new("Delete")
                        .variant(ButtonVariant::Danger)
                        .disabled(disabled)
                        .on_press(|s: &mut Ui| answer_delete(s, true))
                        .build(k)
                        .key("btn-delete-profile-yes"),
                ),
        );
    Dialog::new("dlg-del-profile", "Delete profile")
        .width(CONFIRM_WIDTH)
        .fit()
        .open(true)
        .on_close(|s: &mut Ui| answer_delete(s, false))
        .child(body)
        .build(k, layer);
}
