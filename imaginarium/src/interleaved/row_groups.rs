//! A kernel's step over one group of a row, and the row driven through it to its end, its last
//! partial group included: [`GroupStep::run_row`] from one row into another, [`zip`] from two,
//! [`in_place`], and [`fill`] from none.

use simd::Isa;

/// A kernel's step over one group of `S` source and `D` destination elements, each result a
/// function of its own pixel alone: so a row's last, partial group runs through the same step.
pub(crate) trait GroupStep<A, B, const S: usize, const D: usize>
where
    A: Copy + Default,
    B: Copy + Default,
{
    fn step<V: Isa>(isa: V, group: &[A; S], out: &mut [B; D]);

    /// [`Self::step`] over each group of `src` and `dst`, then once over the partial group left,
    /// into a scratch group of which only the new part is copied out: the source is the row's last
    /// whole group, overlapping the one before, or, in a row shorter than a group, the row padded
    /// with the default value. `src` and `dst` hold the same count of groups, and the same part of
    /// one group past them.
    ///
    /// The scratch keeps each destination byte stored once. A store over bytes an earlier store
    /// still holds waits for that one to reach the cache: an overlapping last group stored in
    /// place doubled the time of an `u8 → u16` frame on one Apple core, a memory-bound row each
    /// paying a cache miss.
    #[inline(always)]
    fn run_row<V: Isa>(isa: V, src: &[A], dst: &mut [B]) {
        let (groups, _) = src.as_chunks::<S>();
        let (out_groups, rest) = dst.as_chunks_mut::<D>();
        debug_assert_eq!(
            groups.len(),
            out_groups.len(),
            "a group of each side per step"
        );
        for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
            Self::step(isa, group, out);
        }
        if rest.is_empty() {
            return;
        }
        let mut out = [B::default(); D];
        if let Some(group) = src.last_chunk::<S>() {
            Self::step(isa, group, &mut out);
            rest.copy_from_slice(&out[D - rest.len()..]);
        } else {
            Self::step(isa, &padded(src), &mut out);
            rest.copy_from_slice(&out[..rest.len()]);
        }
    }
}

/// `step` over each group of `src` and `dst` into `out`, then once over the partial group left,
/// as [`GroupStep::run_row`] takes it. `src`, `dst` and `out` are one length.
#[inline(always)]
pub(crate) fn zip<T: Copy + Default, const N: usize>(
    src: &[T],
    dst: &[T],
    out: &mut [T],
    mut step: impl FnMut(&[T; N], &[T; N], &mut [T; N]),
) {
    debug_assert!(
        src.len() == dst.len() && dst.len() == out.len(),
        "three rows of one length"
    );
    let (src_groups, _) = src.as_chunks::<N>();
    let (dst_groups, _) = dst.as_chunks::<N>();
    let (out_groups, rest) = out.as_chunks_mut::<N>();
    for ((src, dst), out) in src_groups.iter().zip(dst_groups).zip(out_groups) {
        step(src, dst, out);
    }
    if rest.is_empty() {
        return;
    }
    let mut out = [T::default(); N];
    if let (Some(src), Some(dst)) = (src.last_chunk::<N>(), dst.last_chunk::<N>()) {
        step(src, dst, &mut out);
        rest.copy_from_slice(&out[N - rest.len()..]);
    } else {
        let [src, dst] = [src, dst].map(padded::<T, N>);
        step(&src, &dst, &mut out);
        rest.copy_from_slice(&out[..rest.len()]);
    }
}

/// `step` over each group of `row` in place, then once over the partial group left, padded with
/// the default value in a scratch group of which only the row's part is copied back: a group
/// overlapping the one before would step its overlap twice.
#[inline(always)]
pub(crate) fn in_place<T: Copy + Default, const N: usize>(
    row: &mut [T],
    mut step: impl FnMut(&mut [T; N]),
) {
    let (groups, rest) = row.as_chunks_mut::<N>();
    for group in groups {
        step(group);
    }
    if rest.is_empty() {
        return;
    }
    let mut group = padded::<T, N>(rest);
    step(&mut group);
    rest.copy_from_slice(&group[..rest.len()]);
}

/// `step` over each group of `out` by its index, then once over the partial group left, into a
/// scratch group of which only the row's part is copied out.
#[inline(always)]
pub(crate) fn fill<T: Copy + Default, const N: usize>(
    out: &mut [T],
    mut step: impl FnMut(usize, &mut [T; N]),
) {
    let (groups, rest) = out.as_chunks_mut::<N>();
    let whole = groups.len();
    for (idx, group) in groups.iter_mut().enumerate() {
        step(idx, group);
    }
    if rest.is_empty() {
        return;
    }
    let mut group = [T::default(); N];
    step(whole, &mut group);
    rest.copy_from_slice(&group[..rest.len()]);
}

/// `part`, shorter than a group, in a group padded with the default value.
#[inline(always)]
fn padded<T: Copy + Default, const N: usize>(part: &[T]) -> [T; N] {
    let mut group = [T::default(); N];
    group[..part.len()].copy_from_slice(part);
    group
}
