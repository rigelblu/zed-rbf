//! The rules for moving one item along a sequence the user can reorder.
//!
//! Two surfaces share them: the dock icon strips, whose order is
//! `workspace_layout.component_order`, and the status bar's zones, whose order is
//! `status_bar.item_order`. They are here rather than duplicated because the rules
//! are subtle in the same way twice — which side of a target a drop lands on, and
//! what a reversed strip does to that — and a second copy is exactly the
//! two-sources-of-truth defect this feature has already had to unpick three times.
//!
//! Every function is pure and takes the displayed sequence, so a caller supplies
//! what it draws and gets back what to write. `drawn_in_reverse` belongs to the
//! surface: `Dock::strip_is_drawn_in_reverse` owns it for the docks, and the status
//! bar reverses its Right zone.

/// The sequence after `moved` is placed where `target` currently sits on their
/// shared strip.
///
/// **Only this dock's keys change places.** They occupy a fixed set of slots in the
/// global sequence, and the rearrangement permutes which key sits in which slot —
/// so a component on another side keeps its exact index even when it sits between
/// the two that moved. Rewriting the sequence by index would not.
///
/// `strip` is the dock's keys **as drawn**, so a drop lands where the user aimed.
/// `drawn_in_reverse` comes from [`Dock::strip_is_drawn_in_reverse`], the single
/// owner of that fact.
///
/// `None` also when either key is off the strip, or when they are the same key —
/// there is nothing to write that the file does not already say.
pub(crate) fn order_after_placing(
    full: &[String],
    strip: &[String],
    drawn_in_reverse: bool,
    moved: &str,
    target: &str,
) -> Option<Vec<String>> {
    if moved == target {
        return None;
    }
    let from = strip.iter().position(|key| key == moved)?;
    let to = strip.iter().position(|key| key == target)?;

    let mut slots: Vec<usize> = strip
        .iter()
        .map(|key| full.iter().position(|id| id == key))
        .collect::<Option<Vec<_>>>()?;

    let mut rearranged = strip.to_vec();
    let key = rearranged.remove(from);
    rearranged.insert(to, key);
    if drawn_in_reverse {
        rearranged.reverse();
    }

    slots.sort_unstable();
    let mut sequence = full.to_vec();
    for (slot, key) in slots.into_iter().zip(rearranged) {
        sequence[slot] = key;
    }
    Some(sequence)
}

/// The sequence after `moved` leaves its own strip and joins the one `target` is
/// on, landing where `target` currently sits.
///
/// The bar is the placement surface, so dragging across a boundary re-places the
/// component rather than being refused — the caller writes its new position too,
/// and this decides where it lands once it is there.
///
/// Unlike [`order_after_placing`] this cannot permute fixed slots: the target
/// strip gains a key and the source strip loses one, so the slot sets themselves
/// change. It works on the global sequence directly instead — lift `moved` out,
/// then set it down beside `target`. `drawn_in_reverse` is the **target** dock's,
/// because display-left is sequence-later on the right and landing on the wrong
/// side of the target is the whole failure mode here.
/// Which side of the icon it was dropped on the component comes to rest, **as
/// drawn**. Dropping onto an icon means taking its place, so it is `Before`; the
/// end-of-strip zone is the only `After`, and the only way to name the slot past
/// the last icon.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum LandsOn {
    Before,
    After,
}

pub(crate) fn order_after_joining(
    full: &[String],
    drawn_in_reverse: bool,
    moved: &str,
    target: &str,
    lands: LandsOn,
) -> Option<Vec<String>> {
    let mut sequence = full.to_vec();
    let from = sequence.iter().position(|id| id == moved)?;
    sequence.remove(from);
    // A drop onto its own icon needs no guard: `moved` has just been lifted out,
    // so the lookup below cannot find it and the whole call declines. The same
    // guard *is* load-bearing in `order_after_placing`, where removing and
    // reinserting at one index rebuilds the sequence it started from.
    let at = sequence.iter().position(|id| id == target)?;
    // Display-left is sequence-later on a reversed strip, so the side the user
    // aimed at and the index to insert at agree exactly when the two disagree.
    let after_in_sequence = (lands == LandsOn::After) != drawn_in_reverse;
    sequence.insert(
        if after_in_sequence { at + 1 } else { at },
        moved.to_string(),
    );
    Some(sequence)
}
