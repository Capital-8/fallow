//! Optimizations on regex IR

use crate::budget::{CompileBudget, CompileError, CompileLimit, CompileSite};
use crate::compile_storage::{CompileVec, boxed};
use crate::insn::MAX_CHAR_SET_LENGTH;
use crate::ir::*;
use crate::types::BracketContents;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec::Vec};

/// When unrolling a loop, the largest minimum count we will unroll.
const LOOP_UNROLL_THRESHOLD: usize = 5;

/// How many nodes we will inspect in a loop body before refusing to unroll it.
const UNROLL_BODY_BUDGET: usize = 256;

/// Check whether the body of a loop can be unrolled.
/// The 'budget' indicates the numbers of nodes remaining before we give up.
/// We also skip loops with nested loops.
fn charge(budget: Option<&CompileBudget>, work: usize) -> Result<(), CompileError> {
    if let Some(budget) = budget {
        budget.at(CompileSite::Optimizer).reserve(work, 0)?;
    }
    Ok(())
}

fn is_unrollable(
    node: &Node,
    local: &mut usize,
    budget: Option<&CompileBudget>,
) -> Result<bool, CompileError> {
    charge(budget, 1)?;
    if *local == 0 {
        return Ok(false);
    }
    *local -= 1;
    Ok(match node {
        Node::Loop { .. } | Node::Loop1CharBody { .. } => false,
        Node::Cat(nodes) => {
            for n in nodes {
                if !is_unrollable(n, local, budget)? {
                    return Ok(false);
                }
            }
            true
        }
        Node::Alt(left, right) => {
            is_unrollable(left, local, budget)? && is_unrollable(right, local, budget)?
        }
        Node::CaptureGroup { contents, .. } | Node::LookaroundAssertion { contents, .. } => {
            is_unrollable(contents, local, budget)?
        }
        _ => true,
    })
}

/// Things that a Pass may do.
pub enum PassAction {
    // Do nothing to the given node.
    Keep,

    // Notes that we modified the node in-place.
    Modified,

    // Remove the given node outright, effectively replacing it with empty.
    Remove,

    /// Replace the given node with a new Node.
    Replace(Node),
}

#[derive(Debug)]
struct Pass<'a, F>
where
    F: FnMut(&mut Node, &Walk, Option<&CompileBudget>) -> Result<PassAction, CompileError>,
{
    func: &'a mut F,
    changed: bool,
    unicode: bool,
    budget: Option<&'a CompileBudget>,
}

impl<'a, F> Pass<'a, F>
where
    F: FnMut(&mut Node, &Walk, Option<&CompileBudget>) -> Result<PassAction, CompileError>,
{
    fn run_postorder(&mut self, start: &mut Node) -> Result<(), CompileError> {
        try_walk_mut(
            true,
            self.unicode,
            start,
            self.budget,
            CompileSite::Optimizer,
            &mut |n, walk| {
                match (self.func)(n, walk, self.budget)? {
                    PassAction::Keep => {}
                    PassAction::Modified => self.changed = true,
                    PassAction::Remove => {
                        *n = Node::Empty;
                        self.changed = true;
                    }
                    PassAction::Replace(newnode) => {
                        *n = newnode;
                        self.changed = true;
                    }
                }
                Ok(())
            },
        )
    }

    fn run_to_fixpoint(&mut self, n: &mut Node) -> Result<(), CompileError> {
        debug_assert!(!self.changed, "Pass has already been run");
        loop {
            charge(self.budget, 1)?;
            self.changed = false;
            self.run_postorder(n)?;
            if !self.changed {
                return Ok(());
            }
        }
    }
}

fn run_pass<F>(
    r: &mut Regex,
    budget: Option<&CompileBudget>,
    func: &mut F,
) -> Result<bool, CompileError>
where
    F: FnMut(&mut Node, &Walk, Option<&CompileBudget>) -> Result<PassAction, CompileError>,
{
    let mut p = Pass {
        func,
        changed: false,
        unicode: r.flags.unicode,
        budget,
    };
    p.run_to_fixpoint(&mut r.node)?;
    Ok(p.changed)
}

// Here are some optimizations we support.

// Remove empty Nodes.
fn remove_empties(
    n: &mut Node,
    _w: &Walk,
    budget: Option<&CompileBudget>,
) -> Result<PassAction, CompileError> {
    Ok(match n {
        Node::Empty | Node::Goal | Node::Char { .. } => PassAction::Keep,
        Node::ByteSequence(v) => {
            if v.is_empty() {
                PassAction::Remove
            } else {
                PassAction::Keep
            }
        }

        // Note: do not remove empty sets. These always match against one character; an empty set
        // should just fail.
        Node::ByteSet(..) | Node::CharSet(..) | Node::StringSet { .. } => PassAction::Keep,
        Node::Cat(nodes) => {
            charge(budget, nodes.len())?;
            charge(budget, nodes.len())?;
            let blen = nodes.len();
            nodes.retain(|nn| !nn.is_empty());
            if nodes.len() == blen {
                // Nothing was removed.
                PassAction::Keep
            } else {
                match nodes.len() {
                    0 => PassAction::Remove,
                    1 => PassAction::Replace(nodes.pop().unwrap()),
                    _ => PassAction::Modified,
                }
            }
        }
        Node::Alt(left, right) => {
            // Empty alt may match the empty string.
            // Remove it only if both sides are empty.
            if left.is_empty() && right.is_empty() {
                PassAction::Remove
            } else {
                PassAction::Keep
            }
        }
        Node::MatchAny | Node::MatchAnyExceptLineTerminator | Node::Anchor { .. } => {
            PassAction::Keep
        }
        Node::Loop {
            quant,
            loopee,
            enclosed_groups,
        } => {
            // A loop is empty if it has an empty body, or 0 max iters.
            // But do not remove contained capture groups.
            if loopee.is_empty()
                || (quant.max == Some(0) && enclosed_groups.start == enclosed_groups.end)
            {
                PassAction::Remove
            } else {
                PassAction::Keep
            }
        }
        Node::Loop1CharBody { .. } => PassAction::Keep,
        Node::CaptureGroup { .. } => {
            // Capture groups could in principle be optimized if they only match empties.
            PassAction::Keep
        }
        Node::WordBoundary { .. } | Node::BackRef { .. } | Node::Bracket { .. } => PassAction::Keep,
        Node::LookaroundAssertion {
            negate, contents, ..
        } => {
            // Negative arounds that match empties could in principle be optimized to always
            // fail. Here we only optimize positive ones.
            if !*negate && contents.is_empty() {
                PassAction::Remove
            } else {
                PassAction::Keep
            }
        }
    })
}

/// Check if a node contains any capture groups (direct or nested)
fn contains_capture_groups(
    node: &Node,
    budget: Option<&CompileBudget>,
) -> Result<bool, CompileError> {
    charge(budget, 1)?;
    Ok(match node {
        Node::CaptureGroup { .. } => true,
        Node::Cat(nodes) => {
            for n in nodes {
                if contains_capture_groups(n, budget)? {
                    return Ok(true);
                }
            }
            false
        }
        Node::Alt(left, right) => {
            contains_capture_groups(left, budget)? || contains_capture_groups(right, budget)?
        }
        Node::Loop { loopee, .. } => contains_capture_groups(loopee, budget)?,
        Node::LookaroundAssertion { contents, .. } => contains_capture_groups(contents, budget)?,
        _ => false,
    })
}

// If a node can never match, replace it with an always fails node.
fn propagate_early_fails(
    n: &mut Node,
    _w: &Walk,
    budget: Option<&CompileBudget>,
) -> Result<PassAction, CompileError> {
    // Don't optimize nodes containing capture groups to preserve user-visible group numbers
    if contains_capture_groups(n, budget)? {
        return Ok(PassAction::Keep);
    }

    Ok(match n {
        Node::Cat(nodes) => {
            // If any child is an early fail, we are an early fail.
            // Note this assumes that there is no node after a Goal node.
            charge(budget, nodes.len())?;
            if nodes.iter().any(|nn| nn.match_always_fails()) {
                PassAction::Replace(Node::make_always_fails())
            } else {
                PassAction::Keep
            }
        }
        Node::Alt(left, right) => {
            // If both sides are early fails, we are an early fail.
            let left_fails = left.match_always_fails();
            let right_fails = right.match_always_fails();
            match (left_fails, right_fails) {
                (true, true) => PassAction::Replace(Node::make_always_fails()),
                (false, false) => PassAction::Keep,
                (true, false) | (false, true) => {
                    // Here either our left or right node always fails.
                    // "Steal" the other and return it, replacing us.
                    let mut new_node = Node::Empty;
                    core::mem::swap(
                        &mut new_node,
                        if left_fails { &mut *right } else { &mut *left },
                    );
                    PassAction::Replace(new_node)
                }
            }
        }
        Node::Loop {
            loopee,
            quant,
            enclosed_groups,
        } => {
            if enclosed_groups.start < enclosed_groups.end {
                return Ok(PassAction::Keep);
            }
            // If the loop body always fails, we always fail.
            if quant.min > 0 && loopee.match_always_fails() {
                PassAction::Replace(Node::make_always_fails())
            } else {
                PassAction::Keep
            }
        }
        _ => PassAction::Keep,
    })
}

// Remove excess cats.
fn decat(
    n: &mut Node,
    _w: &Walk,
    budget: Option<&CompileBudget>,
) -> Result<PassAction, CompileError> {
    let Node::Cat(nodes) = n else {
        return Ok(PassAction::Keep);
    };
    if nodes.is_empty() {
        return Ok(PassAction::Remove);
    }
    if nodes.len() == 1 {
        return Ok(PassAction::Replace(nodes.pop().unwrap()));
    }
    charge(budget, nodes.len())?;
    if !nodes.iter().any(Node::is_cat) {
        return Ok(PassAction::Keep);
    }
    let mut decatted = CompileVec::new_at(CompileSite::Optimizer);
    let mut length = 0usize;
    for node in nodes.iter() {
        charge(budget, 1)?;
        let extra = match node {
            Node::Cat(children) => children.len(),
            _ => 1,
        };
        length = length.checked_add(extra).ok_or_else(|| {
            budget
                .expect("owned node lengths fit address space")
                .refuse(CompileLimit::Allocation)
        })?;
    }
    decatted.reserve(length, budget)?;
    for node in core::mem::take(nodes) {
        charge(budget, 1)?;
        match node {
            Node::Cat(children) => {
                for child in children {
                    decatted.push(child, budget)?;
                }
            }
            node => decatted.push(node, budget)?,
        }
    }
    Ok(PassAction::Replace(Node::Cat(decatted.into_vec())))
}

// All incoming bounded trees have admitted height, so this scan and failure cleanup
// cannot recurse beyond the implementation ceiling.
fn node_height(node: &Node, budget: &CompileBudget) -> Result<usize, CompileError> {
    charge(Some(budget), 1)?;
    let child = match node {
        Node::Cat(nodes) => {
            let mut height = 0;
            for n in nodes {
                height = height.max(node_height(n, budget)?);
            }
            height
        }
        Node::Alt(left, right) => node_height(left, budget)?.max(node_height(right, budget)?),
        Node::Loop { loopee, .. } | Node::Loop1CharBody { loopee, .. } => {
            node_height(loopee, budget)?
        }
        Node::CaptureGroup { contents, .. } | Node::LookaroundAssertion { contents, .. } => {
            node_height(contents, budget)?
        }
        _ => 0,
    };
    child
        .checked_add(1)
        .ok_or_else(|| budget.refuse(CompileLimit::Depth))
}

// Perform simple unrolling of loops that have a minimum.
fn unroll_loops(
    n: &mut Node,
    walk: &Walk,
    budget: Option<&CompileBudget>,
) -> Result<PassAction, CompileError> {
    let Node::Loop {
        loopee,
        quant,
        enclosed_groups,
    } = n
    else {
        return Ok(PassAction::Keep);
    };
    if enclosed_groups.start < enclosed_groups.end
        || quant.min == 0
        || quant.min > LOOP_UNROLL_THRESHOLD
    {
        return Ok(PassAction::Keep);
    }
    let mut local = UNROLL_BODY_BUDGET;
    if !is_unrollable(loopee, &mut local, budget)? {
        return Ok(PassAction::Keep);
    }
    let remaining = quant.max != Some(quant.min);
    if let Some(budget) = budget {
        let height = node_height(loopee, budget)?;
        let replacement_height = height
            .checked_add(1 + usize::from(remaining))
            .and_then(|height| height.checked_add(walk.depth))
            .ok_or_else(|| budget.refuse(CompileLimit::Depth))?;
        if replacement_height
            > budget
                .max_depth()
                .min(crate::types::MAX_NESTING_DEPTH as usize)
        {
            return Err(budget.refuse(CompileLimit::Depth));
        }
    }
    let mut unrolled = CompileVec::new_at(CompileSite::Optimizer);
    unrolled.reserve(quant.min + usize::from(remaining), budget)?;
    for _ in 0..quant.min {
        let Some(node) = loopee.try_duplicate_with_budget(0, budget)? else {
            return Ok(PassAction::Keep);
        };
        unrolled.push(node, budget)?;
    }
    // Reserve the final move before changing the original loop.
    if remaining {
        charge(budget, 1)?;
    }
    quant.max = quant.max.map(|v| v - quant.min);
    quant.min = 0;
    let mut nodes = unrolled.into_vec();
    if remaining {
        nodes.push(core::mem::replace(n, Node::Empty));
    }
    *n = Node::Cat(nodes);
    Ok(PassAction::Modified)
}

/// Replace Loops with 1Char loops whenever possible.
fn promote_1char_loops(
    n: &mut Node,
    _w: &Walk,
    budget: Option<&CompileBudget>,
) -> Result<PassAction, CompileError> {
    Ok(match n {
        Node::Loop {
            loopee,
            quant,
            enclosed_groups,
        } => {
            // Must be 1Char.
            if !loopee.matches_exactly_one_char() {
                return Ok(PassAction::Keep);
            }

            // The above check should be sufficient to ensure we have no enclosed groups.
            assert!(
                enclosed_groups.start >= enclosed_groups.end,
                "Should have no enclosed groups"
            );

            // This feels hackish?
            let mut new_loopee = boxed(Node::Empty, budget, CompileSite::Optimizer)?;
            core::mem::swap(&mut new_loopee, loopee);

            *n = Node::Loop1CharBody {
                loopee: new_loopee,
                quant: *quant,
            };
            PassAction::Modified
        }
        _ => PassAction::Keep,
    })
}

/// Replace Cat(Char) with ByteSeq.
/// Also replace chars with literal bytes.
/// Don't do this in utf16 mode because UTF-16 should never match against bytes.
/// TODO: this seems to do too much; consider breaking this up.
#[cfg(not(feature = "utf16"))]
fn form_literal_bytes(n: &mut Node, walk: &Walk) -> PassAction {
    // Helper to return a mutable reference to the nodes of a literal bytes.
    fn get_literal_bytes(n: &mut Node) -> Option<&mut Vec<u8>> {
        match n {
            Node::ByteSequence(v) => Some(v),
            _ => None,
        }
    }
    match n {
        Node::Char { c } => {
            if let Some(c) = char::from_u32(*c) {
                let mut buff = [0; 4];
                PassAction::Replace(Node::ByteSequence(
                    c.encode_utf8(&mut buff).as_bytes().to_vec(),
                ))
            } else {
                PassAction::Keep
            }
        }
        Node::CharSet(chars) if chars.iter().all(|&c| c <= 0x7F) => {
            // All of our chars are ASCII; use a byte set instead.
            PassAction::Replace(Node::ByteSet(chars.iter().map(|&c| c as u8).collect()))
        }
        Node::Cat(nodes) => {
            // Find and merge adjacent ByteSeq.
            let mut modified = false;
            for idx in 1..nodes.len() {
                let (prev_slice, curr_slice) = nodes.split_at_mut(idx);
                match (
                    get_literal_bytes(prev_slice.last_mut().unwrap()),
                    get_literal_bytes(curr_slice.first_mut().unwrap()),
                ) {
                    (Some(prev_bytes), Some(curr_bytes))
                        if !prev_bytes.is_empty() && !curr_bytes.is_empty() =>
                    {
                        if walk.in_lookbehind {
                            // Our characters were already reversed; we need to reverse them again
                            // as literal bytes are always forwards. For
                            // example, if we have (<=ab), then we will get Cat(b, a) but we want
                            // literal bytes "ab".
                            curr_bytes.append(prev_bytes);
                        } else {
                            prev_bytes.append(curr_bytes);
                            core::mem::swap(prev_bytes, curr_bytes);
                        }
                        modified = true;
                    }
                    _ => (),
                }
            }
            if modified {
                PassAction::Modified
            } else {
                PassAction::Keep
            }
        }
        _ => PassAction::Keep,
    }
}

/// Try to reduce a bracket to something simpler.
fn try_reduce_bracket(
    bc: &BracketContents,
    budget: Option<&CompileBudget>,
) -> Result<Option<Node>, CompileError> {
    if bc.invert {
        return Ok(None);
    }
    let mut cps_count = 0;
    for iv in bc.cps.intervals() {
        charge(budget, 1)?;
        cps_count += iv.count_codepoints();
    }
    if cps_count > MAX_CHAR_SET_LENGTH {
        return Ok(None);
    }
    let mut res = CompileVec::new_at(CompileSite::Optimizer);
    res.reserve(cps_count, budget)?;
    for iv in bc.cps.intervals() {
        charge(budget, 1)?;
        for cp in iv.codepoints() {
            res.push(cp, budget)?;
        }
    }
    debug_assert!(res.len() <= MAX_CHAR_SET_LENGTH, "Unexpectedly many chars");
    Ok(Some(Node::CharSet(res.into_vec())))
}

/// Optimize brackets like [a-b].
/// Optimize certain stupid brackets like `[a]` to a single char.
/// Invert a bracket if it would *reduce* the number of ranges.
/// Note we only run this once.
fn simplify_brackets(
    n: &mut Node,
    _walk: &Walk,
    budget: Option<&CompileBudget>,
) -> Result<PassAction, CompileError> {
    Ok(match n {
        Node::Bracket(bc) => {
            if let Some(new_node) = try_reduce_bracket(bc, budget)? {
                return Ok(PassAction::Replace(new_node));
            }

            // TODO: does this ever help anything?
            charge(budget, bc.cps.intervals().len())?;
            if bc.cps.intervals().len() > bc.cps.inverted_interval_count() {
                bc.cps = bc.cps.try_inverted(budget, CompileSite::Optimizer)?;
                bc.invert = !bc.invert;
                PassAction::Modified
            } else {
                PassAction::Keep
            }
        }
        _ => PassAction::Keep,
    })
}

pub fn optimize(r: &mut Regex) {
    try_optimize(r, None).expect("unlimited optimization cannot refuse resources");
}

pub(crate) fn try_optimize(
    r: &mut Regex,
    budget: Option<&CompileBudget>,
) -> Result<(), CompileError> {
    run_pass(r, budget, &mut simplify_brackets)?;
    loop {
        charge(budget, 1)?;
        let mut changed = false;
        changed |= run_pass(r, budget, &mut decat)?;
        changed |= run_pass(r, budget, &mut unroll_loops)?;
        changed |= run_pass(r, budget, &mut promote_1char_loops)?;
        #[cfg(not(feature = "utf16"))]
        {
            changed |= run_pass(r, budget, &mut |node, walk, _| {
                Ok(form_literal_bytes(node, walk))
            })?;
        }
        changed |= run_pass(r, budget, &mut remove_empties)?;
        changed |= run_pass(r, budget, &mut propagate_early_fails)?;
        if !changed {
            return Ok(());
        }
    }
}
