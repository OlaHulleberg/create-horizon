//! `CopycatBlock.getSoundType`: a copycat sounds as its material (stepped on, landed on, placed,
//! broken, dug, hit while mined), as the copycat base without one. The `sound_type` hook takes no host and names
//! no level, so each side keeps the material's sound type by position: the server's from the
//! copycats it shows (`show`), by level; a client's from what its copycats' block entities draw.

use mod_api::{
    BlockPart, BlockSoundType, ClientHost, Flow, LoopContext, LoopSide, PlayHitSoundEvent,
    SoundPlay, SoundTypeEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The sounds of the copycats at a position, by level.
type Sounds = HashMap<[i32; 3], Vec<(usize, BlockSoundType)>>;

thread_local! {
    /// The server's: each copycat's material sound, by position and level.
    static SERVER: RefCell<Sounds> = RefCell::new(HashMap::new());
    /// A client's: each copycat drawn with a material, by position, and the material's text.
    static CLIENT: RefCell<HashMap<[i32; 3], (String, BlockSoundType)>> =
        RefCell::new(HashMap::new());
}

/// The server's copycat at `pos` in level `dim` sounds as `sound` (None: as itself).
pub(super) fn set_server(dim: usize, pos: [i32; 3], sound: Option<BlockSoundType>) {
    SERVER.with_borrow_mut(|m| {
        let list = m.entry(pos).or_default();
        list.retain(|(d, _)| *d != dim);
        if let Some(s) = sound {
            list.push((dim, s));
        }
        if list.is_empty() {
            m.remove(&pos);
        }
    });
}

/// A client drew the copycat at `pos` with material `text` (empty: none); `sound` gives the
/// material's sound type when it is new.
pub(super) fn set_client(
    pos: [i32; 3],
    text: &str,
    sound: impl FnOnce() -> Option<BlockSoundType>,
) {
    CLIENT.with_borrow_mut(|m| {
        if text.is_empty() {
            m.remove(&pos);
            return;
        }
        if m.get(&pos).is_some_and(|(t, _)| t == text) {
            return;
        }
        match sound() {
            Some(s) => m.insert(pos, (text.to_owned(), s)),
            None => m.remove(&pos),
        };
    });
}

pub(super) fn reset_server() {
    SERVER.with_borrow_mut(HashMap::clear);
}

pub(super) fn reset_client() {
    CLIENT.with_borrow_mut(HashMap::clear);
}

/// The sound part on the copycat blocks.
pub(super) struct CopycatSound;

#[mod_api::horizon_part]
impl BlockPart for CopycatSound {
    fn sound_type(&self, context: &dyn LoopContext, event: &mut SoundTypeEvent<'_>) {
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let found = match context.side() {
            // The hook names no level: the first level with a copycat holding a material there.
            LoopSide::Server => {
                SERVER.with_borrow(|m| m.get(&pos).and_then(|l| l.first()).map(|s| s.1.clone()))
            }
            LoopSide::Client => CLIENT.with_borrow(|m| m.get(&pos).map(|s| s.1.clone())),
            _ => None,
        };
        if let Some(s) = found {
            *event.sound = s;
        }
    }

    /// The material's hit sound while the copycat is mined (`MultiPlayerGameMode`: at
    /// `(volume + 1) / 8` and `pitch / 2`).
    fn play_hit_sound(&self, host: &mut dyn ClientHost, event: &mut PlayHitSoundEvent) -> Flow {
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        let Some(s) = CLIENT.with_borrow(|m| m.get(&pos).map(|s| s.1.clone())) else {
            return Flow::Continue;
        };
        let at = pos.map(|v| f64::from(v) + 0.5);
        let play = SoundPlay::at(&s.hit, mod_api::SoundSource::Blocks, at)
            .with_volume((s.volume + 1.0) / 8.0)
            .with_pitch(s.pitch * 0.5);
        host.play_local_sound(&play, 0);
        Flow::Cancel
    }
}
