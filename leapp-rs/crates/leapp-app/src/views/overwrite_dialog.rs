//! The question "overwrite this profile?" that core asks before it starts a session.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit};

use crate::Ui;

const DIALOG_WIDTH: f32 = 460.0;

/// Adds the dialog to `layer` for the first waiting question. Later questions wait their turn.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let Some(profile) = s.pending_confirm() else {
        return;
    };
    // The first child holds the profile name: the tests read it as the text of the dialog.
    let body = col()
        .key("dlg-overwrite")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(
            text(format!(
                "The named profile \"{profile}\" has keys in your credentials file."
            ))
            .font(k.ty.ui)
            .color(k.c.text_base),
        )
        .child(
            text(
                "Leapp RS replaces them while the session is active. \
                 It puts them back when the session stops."
                    .to_string(),
            )
            .font(k.ty.ui)
            .color(k.c.text_muted),
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
                        .on_press(|s: &mut Ui| s.answer_confirm(false))
                        .build(k)
                        .key("btn-overwrite-no"),
                )
                .child(
                    Button::new("Overwrite")
                        .on_press(|s: &mut Ui| s.answer_confirm(true))
                        .build(k)
                        .key("btn-overwrite-yes"),
                ),
        );

    Dialog::new("dlg-ow", "Overwrite profile?")
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(|s: &mut Ui| s.answer_confirm(false))
        .child(body)
        .build(k, layer);
}
