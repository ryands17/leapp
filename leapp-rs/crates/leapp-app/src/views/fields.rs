//! Form fields that more than one dialog uses: the label row, the Named profile select and the
//! region select.

use leapp_core::NamedProfile;
use leapp_core::regions::REGIONS;
use rune::{El, Tx, col, el, row, text};
use rune_kit::{Combobox, ComboboxOption, ComboboxState, Kit, Placement};

use crate::Ui;

pub(crate) const FIELD_WIDTH: f32 = 340.0;
const LABEL_WIDTH: f32 = 150.0;

/// The option ID of "Create profile "<text>"". Its key is `<key>-option-__new__`.
pub(crate) const NEW_PROFILE_OPTION: &str = "__new__";

/// The chosen profile name, and the state of its list (open or closed, and its search text).
#[derive(Clone, Debug, Default)]
pub(crate) struct ProfileChoice {
    /// The name of an existing profile, or a new name that the save creates.
    pub name: String,
    pub combo: ComboboxState,
}

impl ProfileChoice {
    pub(crate) fn new(name: impl Into<String>) -> ProfileChoice {
        ProfileChoice {
            name: name.into(),
            combo: ComboboxState::default(),
        }
    }
}

/// The chosen region, and the state of its list.
#[derive(Clone, Debug, Default)]
pub(crate) struct RegionChoice {
    pub region: String,
    pub combo: ComboboxState,
}

impl RegionChoice {
    pub(crate) fn new(region: impl Into<String>) -> RegionChoice {
        RegionChoice {
            region: region.into(),
            combo: ComboboxState::default(),
        }
    }
}

/// A label on the left, the input on the right, and the error text under the input.
pub(crate) fn field(k: &Kit, label: &str, err_key: &str, err: Option<&str>, input: El) -> El {
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

/// The options of the profile list: every profile, the chosen name if it is a new one, and,
/// when the trimmed search text is not one of them, "Create profile "<text>"".
fn profile_options(choice: &ProfileChoice, profiles: &[NamedProfile]) -> Vec<ComboboxOption> {
    let mut options: Vec<ComboboxOption> = profiles
        .iter()
        .map(|p| ComboboxOption::new(p.name.as_str(), p.name.as_str()))
        .collect();
    let known = |options: &[ComboboxOption], name: &str| options.iter().any(|o| o.id == name);
    if !choice.name.is_empty() && !known(&options, &choice.name) {
        options.push(ComboboxOption::new(
            choice.name.as_str(),
            choice.name.as_str(),
        ));
    }
    let typed = choice.combo.search().text.trim();
    if !typed.is_empty() && !known(&options, typed) {
        options.push(ComboboxOption::new(
            NEW_PROFILE_OPTION,
            format!("Create profile \"{typed}\""),
        ));
    }
    options
}

/// The Named profile select keyed `key`: its trigger is `<key>-value-trigger` and its options
/// are `<key>-option-<name>`. Typed text that is not a profile adds the option
/// `<key>-option-__new__`, which chooses the typed name (trimmed). `on` gives the choice in the
/// state; `chosen` runs after each choice (for example, to clear the field's error).
pub(crate) fn profile_field(
    key: &str,
    choice: &ProfileChoice,
    profiles: &[NamedProfile],
    on: impl Fn(&mut Ui) -> &mut ProfileChoice + Clone + 'static,
    chosen: impl Fn(&mut Ui) + 'static,
    k: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let options = profile_options(choice, profiles);
    let combo = on.clone();
    let select = Combobox::new(key, &choice.combo, &options, move |s: &mut Ui| {
        &mut combo(s).combo
    })
    .current(Some(&choice.name))
    .width(FIELD_WIDTH)
    .placement(Placement::BottomStart)
    .label("Named Profile")
    .search_placeholder("Search or type a new profile")
    .empty_message("No profile matches")
    .on_choose(move |s: &mut Ui, id: &str, _tx: &mut Tx| {
        let choice = on(s);
        choice.name = if id == NEW_PROFILE_OPTION {
            choice.combo.search().text.trim().to_string()
        } else {
            id.to_string()
        };
        chosen(s);
    })
    .build(k, layer);
    el().key(key).child(select)
}

/// The region select keyed `key`: its trigger is `<key>-value-trigger` and its options are
/// `<key>-option-<region>`. `on` gives the choice in the state; `chosen` runs after each choice.
pub(crate) fn region_field(
    key: &str,
    choice: &RegionChoice,
    on: impl Fn(&mut Ui) -> &mut RegionChoice + Clone + 'static,
    chosen: impl Fn(&mut Ui) + 'static,
    k: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let regions: Vec<ComboboxOption> = REGIONS
        .iter()
        .map(|r| ComboboxOption::new(*r, *r))
        .collect();
    let combo = on.clone();
    let select = Combobox::new(key, &choice.combo, &regions, move |s: &mut Ui| {
        &mut combo(s).combo
    })
    .current(Some(&choice.region))
    .width(FIELD_WIDTH)
    .placement(Placement::BottomStart)
    .label("Region")
    .search_placeholder("Search regions")
    .empty_message("No region matches")
    .on_choose(move |s: &mut Ui, region: &str, _tx: &mut Tx| {
        on(s).region = region.to_string();
        chosen(s);
    })
    .build(k, layer);
    el().key(key).child(select)
}
