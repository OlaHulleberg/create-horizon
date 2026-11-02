// What Create's packages read and write of a stack beyond the hook helpers: a package's address
// and nine stacks of contents (`PACKAGE_ADDRESS`, `PACKAGE_CONTENTS`), kept in
// `minecraft:custom_data` since Horizon adds no component types; stacks to and from NBT for the
// chunk save; a stack as SNBT for the `summon` of a dropped one. Compiled against Horizon by
// `horizon.py check`; the template test swaps it for `tools/fixture/package_body.rs`.
use create_engine::package::{ADDRESS_KEY, CONTENTS_KEY, SLOTS};
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};

const CUSTOM_DATA: &str = "minecraft:custom_data";

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.data().and_then(|d| d.other(CUSTOM_DATA)) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

fn set_custom(stack: &mut ItemStack, key: &str, value: Option<Tag>) {
    let mut c = custom(stack).cloned().unwrap_or_default();
    match value {
        Some(v) => {
            c.insert(key.into(), v);
        }
        None => {
            c.remove(&horizon_world::nbt::JavaString::from(key));
        }
    }
    let data = stack.data_mut();
    data.set_other(CUSTOM_DATA, (!c.is_empty()).then_some(Tag::Compound(c)));
    stack.tidy();
}

/// `PackageItem.getAddress`: "" when none.
pub(super) fn address(stack: &ItemStack) -> String {
    match custom(stack).and_then(|c| c.get(&ADDRESS_KEY.into())) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

/// `clearAddress`, then `addAddress` when one is given.
pub(super) fn set_address(stack: &mut ItemStack, address: Option<&str>) {
    set_custom(stack, ADDRESS_KEY, address.map(|a| Tag::String(a.into())));
}

/// `getContents`: the nine slots.
pub(super) fn contents(stack: &ItemStack) -> Vec<Option<ItemStack>> {
    horizon_items::item_nbt::decode_items(
        custom(stack).and_then(|c| c.get(&CONTENTS_KEY.into())),
        SLOTS,
    )
}

/// `PACKAGE_CONTENTS` set (`ItemHelper.containerContentsFromHandler`).
pub(super) fn set_contents(stack: &mut ItemStack, slots: &[Option<ItemStack>]) {
    set_custom(
        stack,
        CONTENTS_KEY,
        Some(horizon_items::item_nbt::encode_items(slots)),
    );
}

/// `ItemStack.CODEC`.
pub(super) fn to_nbt(stack: &ItemStack) -> Tag {
    horizon_items::item_nbt::item_to_nbt(stack)
}

pub(super) fn from_nbt(tag: &Tag) -> Option<ItemStack> {
    horizon_items::item_nbt::item_from_nbt(tag)
}

/// The stack as SNBT, for `summon minecraft:item ... {Item:...}`.
pub(super) fn snbt(stack: &ItemStack) -> String {
    horizon_world::snbt_text::to_snbt(&to_nbt(stack))
}
