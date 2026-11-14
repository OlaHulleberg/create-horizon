//! The schematicannon's screen (`SchematicannonScreen`): Create's panel round the menu's five
//! slots, the printing progress, the gunpowder bar, the status line, play, pause and stop, and
//! (behind the settings button) the replace modes and the two skip options. What it shows
//! comes in the block entity's menu data slots (`data`), set by the server after each of the
//! cannon's ticks; its buttons are menu buttons (`ConfigureSchematicannonPacket`).

use create_engine::schematicannon::{Cannon, Configure, SHOTS_PER_GUNPOWDER, State, Status};
use mod_api::{ClientHost, DrawContext, Screen};
use std::cell::Cell;

const TEX: &str = "create:gui/schematics_2";
const WIDGETS: &str = "create:gui/widgets";
const ICONS: &str = "create:gui/icons";
const INVENTORY: &str = "create:gui/player_inventory";

/// What the cannon's screen shows of `c`, its data slots: state, replace mode, skip missing,
/// replace block entities, progress in thousandths, fuel (shots), status, a creative crate.
pub(super) fn cannon_data(c: &Cannon) -> Vec<i32> {
    let progress = if c.blocks_to_place > 0 {
        (i64::from(c.blocks_placed) * 1000 / i64::from(c.blocks_to_place)).clamp(0, 1000) as i32
    } else {
        0
    };
    vec![
        State::ALL.iter().position(|s| *s == c.state).unwrap_or(0) as i32,
        i32::from(c.options.replace_mode),
        i32::from(c.options.skip_missing),
        i32::from(c.options.replace_block_entities),
        progress,
        c.fuel.clamp(0, i32::from(i16::MAX)),
        Status::ALL.iter().position(|s| *s == c.status).unwrap_or(0) as i32,
        i32::from(c.creative),
    ]
}

/// `SchematicannonMenu`: its five slots, the player's inventory at (37, 161), in a window of
/// `SCHEMATICANNON_TOP` and `_BOTTOM` with the inventory under them.
pub(super) fn cannon_screen() -> Screen {
    Screen {
        width: 213.0,
        height: 251.0,
        slots: vec![
            Some([15.0, 65.0]),
            Some([171.0, 65.0]),
            Some([134.0, 19.0]),
            Some([174.0, 19.0]),
            Some([15.0, 19.0]),
        ],
        inventory_y: 161.0,
        inventory_x: 37.0,
        own_labels: true,
        own_background: true,
        ..Screen::default()
    }
}

thread_local! {
    /// Whether the placement settings are shown (`showSettingsIndicator`, off when opened).
    static SETTINGS: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn reset() {
    SETTINGS.set(false);
}

/// A button: its id, `[x, y]` in the window, its icon in `icons.png`.
struct Button {
    id: i32,
    at: [f32; 2],
    icon: [f32; 2],
}

/// Ids of the menu buttons.
pub(super) const PLAY: i32 = 0;
pub(super) const PAUSE: i32 = 1;
pub(super) const STOP: i32 = 2;
pub(super) const MODE: i32 = 10;
pub(super) const SKIP_MISSING: i32 = 20;
pub(super) const SKIP_BLOCK_ENTITIES: i32 = 21;
/// The confirm button: the server closes the menu (`player.closeContainer`).
pub(super) const CONFIRM: i32 = 30;
/// The settings' toggle, the client's own.
const SETTINGS_BUTTON: i32 = -2;

fn buttons() -> Vec<Button> {
    let mut b = vec![
        Button {
            id: PLAY,
            at: [75.0, 85.0],
            icon: [0.0, 80.0],
        },
        Button {
            id: PAUSE,
            at: [93.0, 85.0],
            icon: [16.0, 80.0],
        },
        Button {
            id: STOP,
            at: [111.0, 85.0],
            icon: [32.0, 80.0],
        },
        Button {
            id: CONFIRM,
            at: [180.0, 111.0],
            icon: [0.0, 16.0],
        },
        Button {
            id: SETTINGS_BUTTON,
            at: [8.0, 111.0],
            icon: [48.0, 80.0],
        },
    ];
    if SETTINGS.get() {
        for i in 0..4 {
            b.push(Button {
                id: MODE + i,
                at: [33.0 + i as f32 * 18.0, 111.0],
                icon: [i as f32 * 16.0, 32.0],
            });
        }
        b.push(Button {
            id: SKIP_MISSING,
            at: [111.0, 111.0],
            icon: [16.0, 48.0],
        });
        b.push(Button {
            id: SKIP_BLOCK_ENTITIES,
            at: [135.0, 111.0],
            icon: [32.0, 48.0],
        });
    }
    b
}

fn get(data: &[i32], i: usize) -> i32 {
    data.get(i).copied().unwrap_or(0)
}

/// Whether a button can be pressed, and whether it shows green (`IconButton.green`).
fn looks(b: &Button, data: &[i32]) -> (bool, bool) {
    let state = get(data, 0);
    match b.id {
        PLAY => (state != 2, false),
        PAUSE => (state == 2, false),
        STOP => (state != 0, false),
        m if (MODE..MODE + 4).contains(&m) => (true, get(data, 1) == m - MODE),
        SKIP_MISSING => (true, get(data, 2) != 0),
        SKIP_BLOCK_ENTITIES => (true, get(data, 3) == 0),
        _ => (true, false),
    }
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// `renderBg` and the widgets: everything under the items.
pub(super) fn draw_cannon(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    data: &[i32],
    origin: [f32; 2],
) {
    let [x, y] = origin;
    // The player's inventory, centred under the panel (`renderPlayerInventory`).
    draw.sprite(
        INVENTORY,
        [0.0, 0.0, 176.0, 108.0],
        [x + 29.0, y + 144.0, 176.0, 108.0],
        None,
    );
    let inventory = host
        .translate("container.inventory")
        .unwrap_or_else(|| "Inventory".into());
    draw.text(&inventory, [x + 37.0, y + 150.0], 0xFF40_4040, false);
    draw.sprite(TEX, [0.0, 77.0, 213.0, 42.0], [x, y, 213.0, 42.0], None);
    draw.sprite(
        TEX,
        [0.0, 119.0, 213.0, 99.0],
        [x, y + 42.0, 213.0, 99.0],
        None,
    );
    draw.sprite(
        TEX,
        [0.0, 0.0, 205.0, 15.0],
        [x, y - 2.0, 205.0, 15.0],
        None,
    );
    // `renderPrintingProgress`, `renderFuelBar`.
    let progress = (get(data, 4) as f32 / 1000.0).clamp(0.0, 1.0);
    let w = (114.0 * progress).floor();
    draw.sprite(
        TEX,
        [76.0, 239.0, w, 16.0],
        [x + 44.0, y + 64.0, w, 16.0],
        None,
    );
    if get(data, 7) != 0 {
        draw.sprite(
            TEX,
            [28.0, 239.0, 47.0, 16.0],
            [x + 36.0, y + 19.0, 47.0, 16.0],
            None,
        );
    } else {
        let fuel = (get(data, 5) as f32 / SHOTS_PER_GUNPOWDER as f32).clamp(0.0, 1.0);
        let w = (47.0 * fuel).floor();
        draw.sprite(
            TEX,
            [28.0, 222.0, w, 16.0],
            [x + 36.0, y + 19.0, w, 16.0],
            None,
        );
    }
    let title = host
        .translate("block.create.schematicannon")
        .unwrap_or_else(|| "Schematicannon".into());
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [x + ((213.0 - 8.0 - tw) / 2.0).floor(), y + 2.0],
        0xFF50_5050,
        false,
    );
    // The status line.
    let status = Status::ALL
        .get(get(data, 6).max(0) as usize)
        .copied()
        .unwrap_or_default();
    let key = format!("create.schematicannon.status.{}", status.name());
    let msg = host.translate(&key).unwrap_or(key);
    let mw = draw.text_width(&msg);
    draw.text(
        &msg,
        [x + 103.0 - (mw / 2.0).floor(), y + 53.0],
        0xFFDD_EEFF,
        true,
    );
    if let Some(cannon) = mod_api::Identifier::parse("create:schematicannon")
        .ok()
        .and_then(|i| host.item_key(&i))
    {
        draw.item(cannon, [x + 213.0, y + 141.0 - 48.0], 80.0);
    }
    // The buttons and the play, pause and stop indicators.
    let cursor = host.cursor();
    let state = get(data, 0);
    for (id, ix, lit) in [
        (PLAY, 75.0, (state == 2).then_some(36.0)),
        (PAUSE, 93.0, (state == 1).then_some(54.0)),
        (STOP, 111.0, (state == 0).then_some(72.0)),
    ] {
        let _ = id;
        let src = lit.unwrap_or(0.0);
        draw.sprite(
            WIDGETS,
            [src, 18.0, 18.0, 6.0],
            [x + ix, y + 79.0, 18.0, 6.0],
            None,
        );
    }
    for b in buttons() {
        let r = [x + b.at[0], y + b.at[1], 18.0, 18.0];
        let (active, green) = looks(&b, data);
        let bg = if !active {
            90.0
        } else if green {
            72.0
        } else if inside(cursor, r) {
            18.0
        } else {
            0.0
        };
        draw.sprite(WIDGETS, [bg, 0.0, 18.0, 18.0], r, None);
        draw.sprite(
            ICONS,
            [b.icon[0], b.icon[1], 16.0, 16.0],
            [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
            None,
        );
    }
}

/// A press on the screen: the menu button pressed, if any; the client's own buttons act here.
pub(super) fn click_cannon(host: &dyn ClientHost, data: &[i32], at: [f32; 2]) -> Option<i32> {
    for b in buttons() {
        if !inside(at, [b.at[0], b.at[1], 18.0, 18.0]) {
            continue;
        }
        let (active, green) = looks(&b, data);
        if !active {
            return None;
        }
        host.play_local_sound(&mod_api::SoundPlay::ui("minecraft:ui.button.click"), 0);
        return match b.id {
            SETTINGS_BUTTON => {
                SETTINGS.set(!SETTINGS.get());
                None
            }
            // A mode already chosen sends nothing (`if (replaceMode != mode)`).
            m if (MODE..MODE + 4).contains(&m) && green => None,
            id => Some(id),
        };
    }
    None
}

/// The tooltips of the buttons, the fuel bar and the empty slots.
pub(super) fn tooltip_cannon(
    host: &dyn ClientHost,
    data: &[i32],
    at: [f32; 2],
    lines: &mut Vec<String>,
) {
    let tr = |key: &str| host.translate(key).unwrap_or_else(|| key.to_owned());
    if inside(at, [36.0, 19.0, 47.0, 16.0]) {
        if get(data, 7) != 0 {
            lines.push(tr("create.gui.schematicannon.gunpowderLevel").replace("%1$s", "100"));
        } else {
            let fuel = get(data, 5);
            let percent = fuel * 100 / SHOTS_PER_GUNPOWDER;
            lines.push(
                tr("create.gui.schematicannon.gunpowderLevel")
                    .replace("%1$s", &percent.to_string()),
            );
            lines.push(format!(
                "\u{a7}7{}",
                tr("create.gui.schematicannon.shotsRemaining")
                    .replace("%1$s", &format!("\u{a7}9{fuel}\u{a7}7"))
            ));
        }
        return;
    }
    let keys: [(i32, &str); 7] = [
        (SETTINGS_BUTTON, "create.gui.schematicannon.showOptions"),
        (MODE, "create.gui.schematicannon.option.dontReplaceSolid"),
        (
            MODE + 1,
            "create.gui.schematicannon.option.replaceWithSolid",
        ),
        (MODE + 2, "create.gui.schematicannon.option.replaceWithAny"),
        (
            MODE + 3,
            "create.gui.schematicannon.option.replaceWithEmpty",
        ),
        (SKIP_MISSING, "create.gui.schematicannon.option.skipMissing"),
        (
            SKIP_BLOCK_ENTITIES,
            "create.gui.schematicannon.option.skipBlockEntities",
        ),
    ];
    for b in buttons() {
        if inside(at, [b.at[0], b.at[1], 18.0, 18.0])
            && let Some((_, key)) = keys.iter().find(|(id, _)| *id == b.id)
        {
            lines.push(tr(key));
            if b.id >= MODE {
                let (_, green) = looks(&b, data);
                let (key, colour) = if green {
                    ("create.gui.schematicannon.optionEnabled", "\u{a7}2")
                } else {
                    ("create.gui.schematicannon.optionDisabled", "\u{a7}c")
                };
                lines.push(format!("{colour}{}", tr(key)));
            }
            return;
        }
    }
    let slots = [
        ([15.0, 65.0], "create.gui.schematicannon.slot.schematic"),
        ([134.0, 19.0], "create.gui.schematicannon.slot.listPrinter"),
        ([15.0, 19.0], "create.gui.schematicannon.slot.gunpowder"),
    ];
    for (p, key) in slots {
        if inside(at, [p[0], p[1], 16.0, 16.0]) {
            lines.push(tr(key));
        }
    }
}

/// `ConfigureSchematicannonPacket`: what a menu button asks of the cannon.
pub(super) fn configure(id: i32, data: &[i32]) -> Option<Configure> {
    Some(match id {
        PLAY => Configure::Play,
        PAUSE => Configure::Pause,
        STOP => Configure::Stop,
        m if (MODE..MODE + 4).contains(&m) => Configure::Mode((m - MODE) as u8),
        SKIP_MISSING => Configure::SkipMissing(get(data, 2) == 0),
        SKIP_BLOCK_ENTITIES => Configure::ReplaceBlockEntities(get(data, 3) == 0),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// The schematic table

/// `SchematicTableMenu`: the input and output slots (as the hopper's first and last), the
/// player's inventory at (38, 107), under `SCHEMATIC_TABLE`.
pub(super) fn table_screen() -> Screen {
    Screen {
        width: 214.0,
        height: 197.0,
        slots: vec![Some([21.0, 59.0]), Some([166.0, 59.0]), None, None, None],
        inventory_y: 107.0,
        inventory_x: 38.0,
        own_labels: true,
        own_background: true,
        ..Screen::default()
    }
}

thread_local! {
    /// The schematic chosen in the list (`SelectionScrollInput`'s state).
    static CHOSEN: Cell<usize> = const { Cell::new(0) };
}

/// The upload button's menu id: this plus the chosen schematic's place in the list.
pub(super) const UPLOAD: i32 = 100;

/// The table's screen (`SchematicTableScreen.renderBg` and its widgets): the list of the
/// schematics saved, the upload button and the progress (whole once the output holds one).
pub(super) fn draw_table(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    data: &[i32],
    origin: [f32; 2],
) {
    let _ = data;
    let [x, y] = origin;
    let tr = |key: &str, fallback: &str| host.translate(key).unwrap_or_else(|| fallback.to_owned());
    draw.sprite(
        INVENTORY,
        [0.0, 0.0, 176.0, 108.0],
        [x + 30.0, y + 89.0, 176.0, 108.0],
        None,
    );
    let inventory = tr("container.inventory", "Inventory");
    draw.text(&inventory, [x + 38.0, y + 95.0], 0xFF40_4040, false);
    draw.sprite(
        "create:gui/schematics",
        [10.0, 139.0, 214.0, 85.0],
        [x, y, 214.0, 85.0],
        None,
    );
    let files = super::body::local_files(host);
    let finished = output_filled(host);
    let title = if finished {
        tr("create.gui.schematicTable.finished", "Upload Finished!")
    } else {
        tr("create.gui.schematicTable.title", "Schematic Table")
    };
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [x + ((214.0 - 8.0 - tw) / 2.0).floor(), y + 4.0],
        0xFF50_5050,
        false,
    );
    if files.is_empty() {
        let none = tr(
            "create.gui.schematicTable.noSchematics",
            "No Schematics Saved",
        );
        draw.text(&none, [x + 54.0, y + 26.0], 0xFFD3_D3D3, true);
    } else {
        let i = CHOSEN.get().min(files.len() - 1);
        let colour = if finished { 0xFFCC_DDFF } else { 0xFFFF_FFFF };
        draw.text(&files[i], [x + 51.0, y + 2.0 + 26.0], colour, true);
    }
    if finished {
        draw.sprite(
            "create:gui/schematics",
            [10.0, 224.0, 84.0, 16.0],
            [x + 70.0, y + 59.0, 84.0, 16.0],
            None,
        );
    }
    let cursor = host.cursor();
    let r = [x + 44.0, y + 2.0 + 56.0, 18.0, 18.0];
    let bg = if finished {
        90.0
    } else if inside(cursor, r) {
        18.0
    } else {
        0.0
    };
    draw.sprite(WIDGETS, [bg, 0.0, 18.0, 18.0], r, None);
    draw.sprite(
        ICONS,
        [0.0, 16.0, 16.0, 16.0],
        [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
        None,
    );
    if let Some(table) = mod_api::Identifier::parse("create:schematic_table")
        .ok()
        .and_then(|i| host.item_key(&i))
    {
        draw.item(table, [x + 214.0, y + 85.0 - 40.0], 48.0);
    }
}

/// Whether the table's output slot holds a schematic (`menu.getSlot(1).hasItem()`), read from
/// the open menu.
fn output_filled(host: &dyn ClientHost) -> bool {
    host.local_player()
        .and_then(|p| p.container_menu(host))
        .and_then(|m| m.slot(host, 1))
        .flatten()
        .is_some()
}

/// The upload button pressed: the chosen schematic's place in the list.
pub(super) fn click_table(host: &dyn ClientHost, at: [f32; 2]) -> Option<i32> {
    if !inside(at, [44.0, 58.0, 18.0, 18.0]) || output_filled(host) {
        return None;
    }
    let files = super::body::local_files(host);
    if files.is_empty() {
        return None;
    }
    host.play_local_sound(&mod_api::SoundPlay::ui("minecraft:ui.button.click"), 0);
    Some(UPLOAD + CHOSEN.get().min(files.len() - 1) as i32)
}

/// The wheel over the list chooses the schematic (`SelectionScrollInput`).
pub(super) fn scroll_table(host: &dyn ClientHost, up: bool) {
    let n = super::body::local_files(host).len();
    if n == 0 {
        return;
    }
    let i = CHOSEN.get().min(n - 1);
    CHOSEN.set(if up { (i + n - 1) % n } else { (i + 1) % n });
}

/// The list's tooltip: its title and the schematics, the chosen one marked.
pub(super) fn tooltip_table(host: &dyn ClientHost, at: [f32; 2], lines: &mut Vec<String>) {
    if !inside(at, [45.0, 21.0, 139.0, 18.0]) {
        return;
    }
    let files = super::body::local_files(host);
    if files.is_empty() {
        return;
    }
    let title = host
        .translate("create.gui.schematicTable.availableSchematics")
        .unwrap_or_else(|| "Available Schematics".into());
    lines.push(format!("\u{a7}6{title}"));
    let chosen = CHOSEN.get().min(files.len() - 1);
    for (i, f) in files.iter().enumerate() {
        lines.push(if i == chosen {
            format!("\u{a7}f-> {f}")
        } else {
            format!("\u{a7}7> {f}")
        });
    }
    let scroll = host
        .translate("create.gui.scrollInput.scrollToSelect")
        .unwrap_or_else(|| "Scroll to Select".into());
    lines.push(format!("\u{a7}8\u{a7}o{scroll}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_send_create_s_options() {
        let running = [2, 2, 0, 1];
        assert_eq!(configure(PAUSE, &running), Some(Configure::Pause));
        assert_eq!(configure(MODE + 3, &running), Some(Configure::Mode(3)));
        // Skip missing off: the button turns it on; replacing block entities on: off.
        assert_eq!(
            configure(SKIP_MISSING, &running),
            Some(Configure::SkipMissing(true))
        );
        assert_eq!(
            configure(SKIP_BLOCK_ENTITIES, &running),
            Some(Configure::ReplaceBlockEntities(false))
        );
        let b = |id| Button {
            id,
            at: [0.0; 2],
            icon: [0.0; 2],
        };
        assert!(!looks(&b(PLAY), &running).0, "running: play is off");
        assert!(!looks(&b(STOP), &[0]).0, "stopped: stop is off");
    }
}
