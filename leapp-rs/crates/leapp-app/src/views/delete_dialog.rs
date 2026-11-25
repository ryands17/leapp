//! The question "delete this session?" before the row menu deletes a session. It names the
//! chained sessions below it, which the delete also deletes.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit};

use crate::Ui;

const DIALOG_WIDTH: f32 = 400.0;

/// The text of the confirm. A second sentence names the chained sessions that the delete also
/// deletes, when there are some.
fn question(name: &str, chained: usize) -> String {
    match chained {
        0 => format!("Delete session {name}?"),
        1 => format!("Delete session {name}? 1 chained session that uses it will also be deleted."),
        n => {
            format!("Delete session {name}? {n} chained sessions that use it will also be deleted.")
        }
    }
}

/// Adds the dialog to `layer` while the user is asked.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let Some(ask) = s.delete_ask() else {
        return;
    };
    let disabled = !ask.armed;
    // The first child holds the question: the tests read it as the text of the dialog.
    let body = col()
        .key("dlg-delete")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(
            text(question(&ask.name, ask.chained))
                .font(k.ty.ui)
                .color(k.c.text_base),
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
                        .on_press(|s: &mut Ui| s.answer_delete(false))
                        .build(k)
                        .key("btn-delete-no"),
                )
                .child(
                    Button::new("Delete")
                        .variant(ButtonVariant::Danger)
                        .disabled(disabled)
                        .on_press(|s: &mut Ui| s.answer_delete(true))
                        .build(k)
                        .key("btn-delete-yes"),
                ),
        );

    Dialog::new("dlg-del", "Delete session")
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(|s: &mut Ui| s.answer_delete(false))
        .child(body)
        .build(k, layer);
}
