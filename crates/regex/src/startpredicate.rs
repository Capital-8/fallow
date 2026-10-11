//! Support for quickly finding potential match locations.
use crate::budget::{CompileBudget, CompileError, CompileSite};
use crate::bytesearch::ByteBitmap;
use crate::codepointset;
use crate::compile_storage::{CompileVec, boxed, copy_slice};
use crate::insn::StartPredicate;
use crate::ir;
use crate::ir::Node;
use crate::util::{add_utf8_first_bytes_to_bitmap, utf8_first_byte};
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec::Vec};
use memchr::memmem;

/// Check if a node is anchored to the start of the line/string.
/// Returns true if the node begins with a StartOfLine anchor.
const BYTE_VALUES: usize = 256;
const UTF8_WIDTHS: usize = 4;

fn charge(budget: Option<&CompileBudget>, work: usize) -> Result<(), CompileError> {
    if let Some(budget) = budget {
        budget.at(CompileSite::StartPredicate).reserve(work, 0)?;
    }
    Ok(())
}

fn is_start_anchored(n: &Node, budget: Option<&CompileBudget>) -> Result<bool, CompileError> {
    charge(budget, 1)?;
    Ok(match n {
        Node::Anchor {
            anchor_type: ir::AnchorType::StartOfLine,
            multiline,
        } => !multiline,
        Node::Cat(nodes) => match nodes.first() {
            Some(node) => is_start_anchored(node, budget)?,
            None => false,
        },
        Node::CaptureGroup { contents, .. } => is_start_anchored(contents, budget)?,
        Node::Alt(left, right) => {
            is_start_anchored(left, budget)? && is_start_anchored(right, budget)?
        }
        _ => false,
    })
}

/// Convert the code point set to a first-byte bitmap.
/// That is, make a list of all of the possible first bytes of every contained
/// code point, and store that in a bitmap.
fn cps_to_first_byte_bitmap(
    input: &codepointset::CodePointSet,
    budget: Option<&CompileBudget>,
) -> Result<Box<ByteBitmap>, CompileError> {
    let mut bitmap = boxed(ByteBitmap::default(), budget, CompileSite::StartPredicate)?;
    for iv in input.intervals() {
        charge(budget, 1 + UTF8_WIDTHS + BYTE_VALUES)?;
        add_utf8_first_bytes_to_bitmap(*iv, &mut bitmap);
    }
    Ok(bitmap)
}

/// The "IR" for a start predicate.
enum AbstractStartPredicate {
    /// No predicate.
    Arbitrary,

    /// Sequence of non-empty bytes.
    Sequence(Vec<u8>),

    /// Set of bytes.
    Set(Box<ByteBitmap>),
}

impl AbstractStartPredicate {
    /// \return the disjunction of two predicates.
    /// That is, a predicate that matches x OR y.
    fn disjunction(x: Self, y: Self, budget: Option<&CompileBudget>) -> Result<Self, CompileError> {
        charge(budget, 1)?;
        Ok(match (x, y) {
            (Self::Arbitrary, _) => Self::Arbitrary,
            (_, Self::Arbitrary) => Self::Arbitrary,

            (Self::Sequence(s1), Self::Sequence(s2)) => {
                // Compute the length of the shared prefix.
                charge(budget, s1.len().min(s2.len()))?;
                charge(budget, s1.len().min(s2.len()))?;
                let shared_len = s1.iter().zip(s2.iter()).take_while(|(a, b)| a == b).count();
                debug_assert!(s1[..shared_len] == s2[..shared_len]);
                if shared_len > 0 {
                    // Use the shared prefix.
                    Self::Sequence(copy_slice(
                        &s1[..shared_len],
                        budget,
                        CompileSite::StartPredicate,
                    )?)
                } else {
                    // Use a set of their first byte.
                    Self::Set(boxed(
                        ByteBitmap::new(&[s1[0], s2[0]]),
                        budget,
                        CompileSite::StartPredicate,
                    )?)
                }
            }

            (Self::Set(mut s1), Self::Set(s2)) => {
                charge(budget, core::mem::size_of::<ByteBitmap>())?;
                s1.bitor(s2.as_ref());
                Self::Set(s1)
            }

            (Self::Set(mut s1), Self::Sequence(s2)) => {
                // Add first byte to set.
                s1.set(s2[0]);
                Self::Set(s1)
            }

            (Self::Sequence(s1), Self::Set(mut s2)) => {
                s2.set(s1[0]);
                Self::Set(s2)
            }
        })
    }

    /// Resolve ourselves to a concrete start predicate.
    fn resolve_to_insn(
        self,
        budget: Option<&CompileBudget>,
    ) -> Result<StartPredicate, CompileError> {
        charge(budget, 1)?;
        Ok(match self {
            Self::Arbitrary => StartPredicate::Arbitrary,
            Self::Sequence(vals) => match vals.len() {
                0 => StartPredicate::Arbitrary,
                1 => StartPredicate::ByteSet1([vals[0]]),
                _ => {
                    // Only the unlimited byte compiler produces multi-byte sequences.
                    debug_assert!(budget.is_none());
                    StartPredicate::ByteSeq(Box::new(memmem::Finder::new(&vals).into_owned()))
                }
            },
            Self::Set(bm) => {
                charge(budget, core::mem::size_of::<ByteBitmap>() + BYTE_VALUES)?;
                match bm.count_bits() {
                    0 => StartPredicate::Arbitrary,
                    1 => StartPredicate::ByteSet1(bm.as_array()),
                    2 => StartPredicate::ByteSet2(bm.as_array()),
                    3 => StartPredicate::ByteSet3(bm.as_array()),
                    _ => StartPredicate::ByteBracket(*bm),
                }
            }
        })
    }
}

/// Compute any start-predicate for a node..
/// If this returns None, then the instruction is conceptually zero-width (e.g.
/// lookahead assertion) and does not contribute to the predicate.
/// If this returns StartPredicate::Arbitrary, then there is no predicate.
fn compute_start_predicate(
    n: &Node,
    budget: Option<&CompileBudget>,
) -> Result<Option<AbstractStartPredicate>, CompileError> {
    charge(budget, 1)?;
    let arbitrary = Some(AbstractStartPredicate::Arbitrary);
    Ok(match n {
        Node::ByteSequence(bytevec) => Some(AbstractStartPredicate::Sequence(copy_slice(
            bytevec,
            budget,
            CompileSite::StartPredicate,
        )?)),
        Node::ByteSet(bytes) => {
            charge(budget, bytes.len())?;
            Some(AbstractStartPredicate::Set(boxed(
                ByteBitmap::new(bytes),
                budget,
                CompileSite::StartPredicate,
            )?))
        }

        Node::Empty => arbitrary,
        Node::Goal => arbitrary,
        Node::BackRef { .. } => arbitrary,

        Node::CharSet(chars) => {
            // Pick the first bytes out.
            let mut bytes = CompileVec::new_at(CompileSite::StartPredicate);
            bytes.reserve(chars.len(), budget)?;
            for &c in chars {
                bytes.push(utf8_first_byte(c), budget)?;
            }
            charge(budget, bytes.len())?;
            Some(AbstractStartPredicate::Set(boxed(
                ByteBitmap::new(&bytes),
                budget,
                CompileSite::StartPredicate,
            )?))
        }

        // StringSets come from TC39 "sequence properties" and are very rare - not worth optimizing.
        Node::StringSet { .. } => arbitrary,

        // We assume that most char nodes have been optimized to ByteSeq or AnyBytes2, so skip
        // these.
        // TODO: we could support icase through bitmap of de-folded first bytes.
        Node::Char { .. } => arbitrary,

        // Cats return the first non-None value, if any.
        Node::Cat(nodes) => {
            for node in nodes {
                if let Some(predicate) = compute_start_predicate(node, budget)? {
                    return Ok(Some(predicate));
                }
            }
            None
        }

        // MatchAny (aka .) is too common to do a fast prefix search for.
        Node::MatchAny => arbitrary,

        // MatchAnyExceptLineTerminator (aka .) is too common to do a fast prefix search for.
        Node::MatchAnyExceptLineTerminator => arbitrary,

        // TODO: can probably exploit some of these.
        Node::Anchor { .. } => arbitrary,
        Node::WordBoundary { .. } => arbitrary,

        // Capture groups delegate to their contents.
        Node::CaptureGroup { contents, .. } => compute_start_predicate(contents, budget)?,

        // Zero-width assertions are one of the few instructions that impose no start predicate.
        Node::LookaroundAssertion { .. } => None,

        Node::Loop { loopee, quant, .. } => {
            // TODO: we could try to join two predicates if the loop were optional.
            if quant.min > 0 {
                compute_start_predicate(loopee, budget)?
            } else {
                arbitrary
            }
        }

        Node::Loop1CharBody { loopee, quant } => {
            // TODO: we could try to join two predicates if the loop were optional.
            if quant.min > 0 {
                compute_start_predicate(loopee, budget)?
            } else {
                arbitrary
            }
        }

        // This one is interesting - we compute the disjunction of the predicates of our two arms.
        Node::Alt(left, right) => {
            if let (Some(x), Some(y)) = (
                compute_start_predicate(left, budget)?,
                compute_start_predicate(right, budget)?,
            ) {
                Some(AbstractStartPredicate::disjunction(x, y, budget)?)
            } else {
                // This indicates that one of our branches could match the empty string.
                arbitrary
            }
        }

        // Brackets get a bitmap.
        Node::Bracket(bc) => {
            // If our bracket is inverted, construct the set of code points not contained.
            let storage;
            let cps = if bc.invert {
                storage = bc.cps.try_inverted(budget, CompileSite::StartPredicate)?;
                &storage
            } else {
                &bc.cps
            };
            let bitmap = cps_to_first_byte_bitmap(cps, budget)?;
            Some(AbstractStartPredicate::Set(bitmap))
        }
    })
}

/// \return the start predicate for a Regex.
pub(crate) fn try_predicate_for_re(
    re: &ir::Regex,
    budget: Option<&CompileBudget>,
) -> Result<StartPredicate, CompileError> {
    if is_start_anchored(&re.node, budget)? && !re.flags.multiline {
        return Ok(StartPredicate::StartAnchored);
    }
    compute_start_predicate(&re.node, budget)?
        .unwrap_or(AbstractStartPredicate::Arbitrary)
        .resolve_to_insn(budget)
}

#[cfg(test)]
mod predicate_budget_tests {
    use super::*;
    use crate::{CompileBudget, CompileError, CompileLimit};

    #[test]
    fn predicate_refuses_unfunded_visit() {
        let ir = crate::parse::try_parse("[α-ω]".chars().map(u32::from), crate::Flags::from("u"))
            .unwrap();
        let budget = CompileBudget::new(0, 1_000_000, 128);
        assert!(matches!(
            try_predicate_for_re(&ir, Some(&budget)),
            Err(CompileError::Limit(CompileLimit::Work))
        ));
    }

    #[test]
    fn predicate_refuses_unfunded_bitmap() {
        let ir = crate::parse::try_parse("[α-ω]".chars().map(u32::from), crate::Flags::from("u"))
            .unwrap();
        let budget = CompileBudget::new(1_000_000, 0, 128);
        assert!(matches!(
            try_predicate_for_re(&ir, Some(&budget)),
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        let control = CompileBudget::new(1_000_000, 1_000_000, 128);
        assert!(try_predicate_for_re(&ir, Some(&control)).is_ok());
    }
}
