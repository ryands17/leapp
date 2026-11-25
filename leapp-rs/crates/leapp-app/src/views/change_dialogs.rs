//! The "Change region…" and "Change named profile…" dialogs of the row menu (spec 14.7).

use leapp_core::{Error, SessionId};
use rune::{El, Tx, col, row};
use rune_kit::{Button, ButtonVariant, Dialog, Kit};

use super::fields::{ProfileChoice, RegionChoice, field, profile_field, region_field};
use crate::{Msg, Ui};

const DIALOG_WIDTH: f32 = 520.0;

/// What a dialog changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum What {
    Region,
    Profile,
}

/// The open dialog: which value of which session.
#[derive(Clone, Copy, Debug)]
struct Target {
    id: SessionId,
    what: What,
}

/// The state of the two change dialogs. At most one is open.
#[derive(Debug, Default)]
pub(crate) struct ChangeState {
    open: Option<Target>,
    region: RegionChoice,
    profile: ProfileChoice,
    /// The token of the save that runs now. Only the result with this token goes to the dialog.
    saving: Option<u64>,
    /// The token of the next save.
    next_token: u64,
}

/// Opens the Change region dialog of session `id` with its region. Ignored while a save runs,
/// or if the session is gone.
pub(crate) fn open_region(s: &mut Ui, id: SessionId) {
    open(s, id, What::Region);
}

/// Opens the Change named profile dialog of session `id` with its profile. Ignored while a save
/// runs, or if the session is gone.
pub(crate) fn open_profile(s: &mut Ui, id: SessionId) {
    open(s, id, What::Profile);
}

fn open(s: &mut Ui, id: SessionId, what: What) {
    if s.change.saving.is_some() {
        return;
    }
    let Some(session) = s.ws.session(id) else {
        return;
    };
    let profile = s.ws.profile_name(session.profile_id).unwrap_or_default();
    s.change.open = Some(Target { id, what });
    s.change.region = RegionChoice::new(session.region.as_str());
    s.change.profile = ProfileChoice::new(profile);
}

/// Closes the dialog. Ignored while a save runs: its result goes to this dialog.
fn cancel(s: &mut Ui) {
    if s.change.saving.is_some() {
        return;
    }
    s.change.open = None;
}

/// Sends the chosen value to core, off the render thread: an active session stops and starts
/// again, and the start can ask a question that this thread must show. [`done`] gets the result.
/// Ignored while a save runs.
fn save(s: &mut Ui, _tx: &mut Tx) {
    let Some(target) = s.change.open else {
        return;
    };
    if s.change.saving.is_some() {
        return;
    }
    let token = s.change.next_token;
    s.change.next_token = token.wrapping_add(1);
    s.change.saving = Some(token);
    let leapp = s.leapp.clone();
    let bridge = s.bridge.clone();
    let id = target.id;
    let task = match target.what {
        What::Region => {
            let region = s.change.region.region.clone();
            s.rt.spawn(async move { leapp.change_region(id, &region).await })
        }
        What::Profile => {
            let profile = s.change.profile.name.clone();
            s.rt.spawn(async move { leapp.change_profile(id, &profile).await })
        }
    };
    // Always send a result, also if the task panics: else the dialog stays locked.
    s.rt.spawn(async move {
        let result = task.await.unwrap_or_else(|_| {
            Err(Error::Io(std::io::Error::other(
                "the change stopped unexpectedly; try again",
            )))
        });
        bridge.send(Msg::ChangeDone { token, result });
    });
}

/// The result of the save with `token`. An error shows in the toast. Success, or an error that
/// is not about the chosen value (a failed restart keeps the change, F23-R4), closes the dialog;
/// a name or region error keeps it open, so the user can choose again. A result of another save
/// only shows its error.
pub(crate) fn done(s: &mut Ui, token: u64, result: leapp_core::Result<()>) {
    let mine = s.change.saving == Some(token);
    if mine {
        s.change.saving = None;
    }
    match result {
        Ok(()) => {
            if mine {
                s.change.open = None;
            }
        }
        Err(e) => {
            if mine && !matches!(e, Error::InvalidInput { .. }) {
                s.change.open = None;
            }
            s.show_error(e.to_string());
        }
    }
}

/// Adds the open change dialog to `layer`.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let st = &s.change;
    let Some(target) = st.open else {
        return;
    };
    // The rune-kit `Dialog` keys its panel `<key>-dialog`: a key that is not `dlg-<key>` keeps
    // `dlg-change-region` and `dlg-change-profile` for the body.
    let (key, dialog_key, title, label, input) = match target.what {
        What::Region => (
            "change-region",
            "dlg-chg-region",
            "Change region",
            "Region",
            region_field(
                "sel-change-region",
                &st.region,
                |s: &mut Ui| &mut s.change.region,
                |_: &mut Ui| {},
                k,
                layer,
            ),
        ),
        What::Profile => (
            "change-profile",
            "dlg-chg-profile",
            "Change named profile",
            "Named Profile",
            profile_field(
                "sel-change-profile",
                &st.profile,
                &s.ws.profiles,
                |s: &mut Ui| &mut s.change.profile,
                |_: &mut Ui| {},
                k,
                layer,
            ),
        ),
    };
    let saving = st.saving.is_some();
    let body = col()
        .key(format!("dlg-{key}"))
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(field(k, label, &format!("err-{key}"), None, input))
        .child(
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
                        .key(format!("btn-{key}-cancel")),
                )
                .child(
                    Button::new("Save")
                        .disabled(saving)
                        .on_press_tx(save)
                        .build(k)
                        .key(format!("btn-{key}-save")),
                ),
        );
    Dialog::new(dialog_key, title)
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(cancel)
        .child(body)
        .build(k, layer);
}
