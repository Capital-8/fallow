//! Parser from regex patterns to IR

use crate::budget::{CompileBudget, CompileError, CompileLimit, CompileSite};
use crate::compile_storage::{
    CompileString, CompileVec, boxed_slice, copy_boxed_slice, copy_slice, copy_string, syntax_error,
};
use crate::{
    api, charclasses,
    codepointset::{CodePoint, CodePointSet, Interval, try_interval_contains},
    insn::MAX_CHAR_SET_LENGTH,
    ir,
    types::{
        BracketContents, CaptureGroupID, CharacterClassType, MAX_CAPTURE_GROUPS, MAX_LOOPS,
        MAX_NESTING_DEPTH,
    },
    unicode::{
        self, PropertyEscapeKind, try_unicode_property_from_str, try_unicode_property_name_from_str,
    },
    unicodetables::{id_continue_ranges, id_start_ranges},
    util::to_char_sat,
};
use core::{fmt, iter::Peekable, mem};
#[cfg(feature = "std")]
#[allow(
    clippy::disallowed_types,
    reason = "Preserve upstream std and alloc map choices for the unlimited compiler."
)]
use std::collections::HashMap;
#[cfg(all(not(feature = "std"), feature = "alloc"))]
use {
    alloc::{boxed::Box, string::String, vec::Vec},
    hashbrown::HashMap,
};

/// Represents an error encountered during regex compilation.
///
/// The text contains a human-readable error message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Error {
    pub text: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

enum ClassAtom {
    CodePoint(CodePoint),
    CharacterClass {
        class_type: CharacterClassType,
        positive: bool,
    },
    Range {
        iv: CodePointSet,
        negate: bool,
    },
}

/// Represents the result of a class set.
#[derive(Debug, Clone)]
struct ClassSet {
    codepoints: CodePointSet,
    alternatives: ClassSetAlternativeStrings,
    // This syntax property survives set operations that remove all actual strings.
    may_contain_strings: bool,
}

impl ClassSet {
    fn new() -> Self {
        ClassSet {
            codepoints: CodePointSet::new(),
            alternatives: ClassSetAlternativeStrings::new(),
            may_contain_strings: false,
        }
    }

    fn prepare_negation(&mut self, budget: Option<&CompileBudget>) -> Result<(), CompileError> {
        if self.may_contain_strings {
            return syntax_error(format_args!("Invalid character escape"), budget);
        }
        for alternative in mem::take(&mut self.alternatives.0) {
            class_work(budget, 1)?;
            // A false MayContainStrings result leaves only single characters.
            self.codepoints.try_add_one(alternative[0], budget)?;
        }
        Ok(())
    }

    fn node(
        mut self,
        icase: bool,
        negate_set: bool,
        budget: Option<&CompileBudget>,
    ) -> Result<ParsedNode, CompileError> {
        if negate_set {
            self.prepare_negation(budget)?;
        }
        let codepoints = if icase {
            unicode::try_add_icase_code_points(self.codepoints, budget)?
        } else {
            self.codepoints
        };
        let codepoints_empty = codepoints.is_empty();
        let bracket = ir::Node::Bracket(BracketContents {
            invert: negate_set,
            cps: codepoints,
        });
        if self.alternatives.0.is_empty() {
            ParsedNode::leaf(bracket, budget)
        } else if codepoints_empty {
            ParsedNode::leaf(self.alternatives.into_node(icase, budget)?, budget)
        } else {
            let mut has_empty = false;
            self.alternatives.filter(budget, |text| {
                has_empty |= text.is_empty();
                Ok(!text.is_empty())
            })?;
            let mut nodes = node_list([], budget)?;
            if !self.alternatives.0.is_empty() {
                nodes.push(
                    ParsedNode::leaf(self.alternatives.into_node(icase, budget)?, budget)?,
                    budget,
                )?;
            }
            nodes.push(ParsedNode::leaf(bracket, budget)?, budget)?;
            if has_empty {
                nodes.push(ParsedNode::leaf(ir::Node::Empty, budget)?, budget)?;
            }
            make_alt(nodes, budget)
        }
    }

    fn try_clone(&self, budget: Option<&CompileBudget>) -> Result<Self, CompileError> {
        Ok(Self {
            codepoints: self.codepoints.try_clone(budget, CompileSite::ClassSet)?,
            alternatives: self.alternatives.try_clone(budget)?,
            may_contain_strings: self.may_contain_strings,
        })
    }

    fn union_operand(
        &mut self,
        operand: ClassSetOperand,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        self.may_contain_strings |= operand.may_contain_strings();
        match operand {
            ClassSetOperand::ClassSetCharacter(c) => self.codepoints.try_add_one(c, budget)?,
            ClassSetOperand::CharacterClassEscape(cps) => {
                self.codepoints.try_add_set(cps, budget)?
            }
            ClassSetOperand::Class(class) => {
                self.codepoints.try_add_set(class.codepoints, budget)?;
                self.alternatives.extend(class.alternatives, budget)?;
            }
            ClassSetOperand::ClassStringDisjunction(s, _) => self.alternatives.extend(s, budget)?,
        }
        Ok(())
    }

    fn intersect_operand(
        &mut self,
        operand: ClassSetOperand,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        self.may_contain_strings &= operand.may_contain_strings();
        match operand {
            ClassSetOperand::ClassSetCharacter(c) => {
                let contains = self.codepoints.try_contains(c, budget)?;
                self.codepoints.clear();
                if contains {
                    self.codepoints.try_add_one(c, budget)?;
                }
                let mut found = false;
                for alternative in &self.alternatives.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1 && alternative[0] == c {
                        found = true;
                        break;
                    }
                }
                class_work(budget, self.alternatives.0.len())?;
                self.alternatives.0.clear();
                if found {
                    self.alternatives.push_copy(&[c], budget)?;
                }
            }
            ClassSetOperand::CharacterClassEscape(cps) => {
                self.codepoints.try_intersect(cps.intervals(), budget)?;
                self.alternatives.filter(budget, |alternative| {
                    Ok(alternative.len() == 1 && cps.try_contains(alternative[0], budget)?)
                })?;
            }
            ClassSetOperand::Class(class) => {
                let mut retained_codepoints = CodePointSet::new();
                for alternative in &class.alternatives.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1
                        && self.codepoints.try_contains(alternative[0], budget)?
                    {
                        retained_codepoints.try_add_one(alternative[0], budget)?;
                    }
                }
                let mut retained_alternatives = ClassSetAlternativeStrings::new();
                for alternative in &self.alternatives.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1
                        && class.codepoints.try_contains(alternative[0], budget)?
                    {
                        retained_alternatives.push_copy(alternative, budget)?;
                    }
                }
                self.codepoints
                    .try_intersect(class.codepoints.intervals(), budget)?;
                self.codepoints.try_add_set(retained_codepoints, budget)?;
                self.alternatives.intersect(&class.alternatives, budget)?;
                self.alternatives.extend(retained_alternatives, budget)?;
            }
            ClassSetOperand::ClassStringDisjunction(s, _) => {
                let mut retained = CodePointSet::new();
                for alternative in &s.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1
                        && self.codepoints.try_contains(alternative[0], budget)?
                    {
                        retained.try_add_one(alternative[0], budget)?;
                    }
                }
                self.codepoints = retained;
                self.alternatives.intersect(&s, budget)?;
            }
        }
        Ok(())
    }

    fn subtract_operand(
        &mut self,
        operand: ClassSetOperand,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        match operand {
            ClassSetOperand::ClassSetCharacter(c) => {
                self.codepoints
                    .try_remove(&[Interval { first: c, last: c }], budget)?;
                self.alternatives.filter(budget, |alternative| {
                    Ok(alternative.len() != 1 || alternative[0] != c)
                })?;
            }
            ClassSetOperand::CharacterClassEscape(cps) => {
                self.codepoints.try_remove(cps.intervals(), budget)?;
                let mut to_remove = ClassSetAlternativeStrings::new();
                for alternative in &self.alternatives.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1 && cps.try_contains(alternative[0], budget)? {
                        to_remove.push_copy(alternative, budget)?;
                    }
                }
                self.alternatives.remove(&to_remove, budget)?;
            }
            ClassSetOperand::Class(class) => {
                let mut removed = CodePointSet::new();
                for alternative in &class.alternatives.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1
                        && self.codepoints.try_contains(alternative[0], budget)?
                    {
                        removed.try_add_one(alternative[0], budget)?;
                    }
                }
                let mut alternatives_removed = ClassSetAlternativeStrings::new();
                for alternative in &self.alternatives.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1
                        && class.codepoints.try_contains(alternative[0], budget)?
                    {
                        alternatives_removed.push_copy(alternative, budget)?;
                    }
                }
                self.codepoints.try_remove(removed.intervals(), budget)?;
                self.codepoints
                    .try_remove(class.codepoints.intervals(), budget)?;
                self.alternatives.remove(&alternatives_removed, budget)?;
                self.alternatives.remove(&class.alternatives, budget)?;
            }
            ClassSetOperand::ClassStringDisjunction(s, _) => {
                let mut removed = CodePointSet::new();
                for alternative in &s.0 {
                    class_work(budget, 1)?;
                    if alternative.len() == 1
                        && self.codepoints.try_contains(alternative[0], budget)?
                    {
                        removed.try_add_one(alternative[0], budget)?;
                    }
                }
                self.codepoints.try_remove(removed.intervals(), budget)?;
                self.alternatives.remove(&s, budget)?;
            }
        }
        Ok(())
    }
}

enum ClassSetOperator {
    Complete,
    Union,
    Intersection,
    Subtraction,
}

/// Represents all different types of class set operands.
#[derive(Debug, Clone)]
enum ClassSetOperand {
    ClassSetCharacter(CodePoint),
    CharacterClassEscape(CodePointSet),
    Class(ClassSet),
    ClassStringDisjunction(ClassSetAlternativeStrings, bool),
}

impl ClassSetOperand {
    fn may_contain_strings(&self) -> bool {
        match self {
            Self::ClassSetCharacter(_) | Self::CharacterClassEscape(_) => false,
            Self::Class(class) => class.may_contain_strings,
            Self::ClassStringDisjunction(_, may_contain_strings) => *may_contain_strings,
        }
    }

    fn try_clone(&self, budget: Option<&CompileBudget>) -> Result<Self, CompileError> {
        Ok(match self {
            Self::ClassSetCharacter(c) => Self::ClassSetCharacter(*c),
            Self::CharacterClassEscape(cps) => {
                Self::CharacterClassEscape(cps.try_clone(budget, CompileSite::ClassSet)?)
            }
            Self::Class(class) => Self::Class(class.try_clone(budget)?),
            Self::ClassStringDisjunction(strings, may_contain_strings) => {
                Self::ClassStringDisjunction(strings.try_clone(budget)?, *may_contain_strings)
            }
        })
    }
}

fn class_work(budget: Option<&CompileBudget>, work: usize) -> Result<(), CompileError> {
    if let Some(budget) = budget {
        budget.at(CompileSite::ClassSet).reserve(work, 0)?;
    }
    Ok(())
}

/// A list of strings matching some property, for use in 'v' regular expressions.
#[derive(Debug, Clone)]
struct ClassSetAlternativeStrings(CompileVec<Box<[CodePoint]>>);

impl ClassSetAlternativeStrings {
    fn new() -> Self {
        Self(CompileVec::new_at(CompileSite::ClassSet))
    }

    fn push_copy(
        &mut self,
        text: &[CodePoint],
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        self.0.push(
            copy_boxed_slice(text, budget, CompileSite::ClassSet)?,
            budget,
        )
    }

    fn from_slices(
        strings: &[&[CodePoint]],
        budget: Option<&CompileBudget>,
    ) -> Result<Self, CompileError> {
        let mut result = Self::new();
        result.0.reserve(strings.len(), budget)?;
        for text in strings {
            result.push_copy(text, budget)?;
        }
        Ok(result)
    }

    fn try_clone(&self, budget: Option<&CompileBudget>) -> Result<Self, CompileError> {
        let mut result = Self::new();
        result.0.reserve(self.0.len(), budget)?;
        for text in &self.0 {
            result.push_copy(text, budget)?;
        }
        Ok(result)
    }

    fn extend(&mut self, other: Self, budget: Option<&CompileBudget>) -> Result<(), CompileError> {
        self.0.reserve(other.0.len(), budget)?;
        for text in other.0 {
            self.0.push(text, budget)?;
        }
        Ok(())
    }

    fn contains(
        other: &[Box<[CodePoint]>],
        text: &[CodePoint],
        budget: Option<&CompileBudget>,
    ) -> Result<bool, CompileError> {
        for candidate in other {
            class_work(budget, 1)?;
            if candidate.len() != text.len() {
                continue;
            }
            let mut equal = true;
            for (left, right) in candidate.iter().zip(text) {
                class_work(budget, 1)?;
                if left != right {
                    equal = false;
                    break;
                }
            }
            if equal {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn filter(
        &mut self,
        budget: Option<&CompileBudget>,
        mut keep: impl FnMut(&[CodePoint]) -> Result<bool, CompileError>,
    ) -> Result<(), CompileError> {
        let mut retained = CompileVec::new_at(CompileSite::ClassSet);
        for text in mem::take(&mut self.0) {
            class_work(budget, 1)?;
            if keep(&text)? {
                retained.push(text, budget)?;
            }
        }
        self.0 = retained;
        Ok(())
    }

    fn intersect(
        &mut self,
        other: &[Box<[CodePoint]>],
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        self.filter(budget, |text| Self::contains(other, text, budget))
    }

    fn remove(
        &mut self,
        other: &[Box<[CodePoint]>],
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        self.filter(budget, |text| Ok(!Self::contains(other, text, budget)?))
    }

    fn into_node(
        self,
        icase: bool,
        budget: Option<&CompileBudget>,
    ) -> Result<ir::Node, CompileError> {
        let mut alternatives = self.0;
        if budget.is_none() {
            #[allow(clippy::unnecessary_sort_by)]
            alternatives.sort_by(|a, b| b.len().cmp(&a.len()));
        } else {
            // Bottom-up stable merge retains priority among equally long strings.
            // Empty boxes are zero-sized placeholders, not copied string payloads.
            let len = alternatives.len();
            let mut scratch = CompileVec::new_at(CompileSite::ClassSet);
            scratch.reserve(len, budget)?;
            for _ in 0..len {
                scratch.push(Box::<[CodePoint]>::default(), budget)?;
            }
            let mut width = 1usize;
            while width < len {
                class_work(budget, 1)?;
                let mut start = 0usize;
                while start < len {
                    let middle = start.saturating_add(width).min(len);
                    let end = middle.saturating_add(width).min(len);
                    let (mut left, mut right) = (start, middle);
                    for out in start..end {
                        class_work(budget, 1)?;
                        let take_left = right == end
                            || (left < middle
                                && alternatives[left].len() >= alternatives[right].len());
                        let input = if take_left {
                            let i = left;
                            left += 1;
                            i
                        } else {
                            let i = right;
                            right += 1;
                            i
                        };
                        class_work(budget, 1)?;
                        scratch[out] = mem::take(&mut alternatives[input]);
                    }
                    start = end;
                }
                mem::swap(&mut alternatives, &mut scratch);
                width = width.saturating_mul(2);
            }
        }
        Ok(ir::Node::StringSet {
            alternatives: alternatives.into_vec(),
            icase,
        })
    }
}

impl core::ops::Deref for ClassSetAlternativeStrings {
    type Target = [Box<[CodePoint]>];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn node_list<const N: usize>(
    nodes: [ParsedNode; N],
    budget: Option<&CompileBudget>,
) -> Result<CompileVec<ParsedNode>, CompileError> {
    let mut result = CompileVec::new();
    for node in nodes {
        result.push(node, budget)?;
    }
    Ok(result)
}

struct ParsedNode {
    node: ir::Node,
    height: usize,
}

impl ParsedNode {
    fn check_height(height: usize, budget: Option<&CompileBudget>) -> Result<(), CompileError> {
        if let Some(budget) = budget {
            budget.at(CompileSite::ParserHeight).reserve(1, 0)?;
            if height > budget.max_depth().min(MAX_NESTING_DEPTH as usize) {
                return Err(budget.refuse(CompileLimit::Depth));
            }
        }
        Ok(())
    }

    fn leaf(node: ir::Node, budget: Option<&CompileBudget>) -> Result<Self, CompileError> {
        Self::check_height(1, budget)?;
        Ok(Self { node, height: 1 })
    }

    fn parent_height(
        child_height: usize,
        budget: Option<&CompileBudget>,
    ) -> Result<usize, CompileError> {
        let Some(budget) = budget else {
            return Ok(0);
        };
        let height = child_height
            .checked_add(1)
            .ok_or_else(|| budget.refuse(CompileLimit::Depth))?;
        Self::check_height(height, Some(budget))?;
        Ok(height)
    }

    fn boxed(self, budget: Option<&CompileBudget>) -> Result<(Box<ir::Node>, usize), CompileError> {
        let height = Self::parent_height(self.height, budget)?;
        if let Some(budget) = budget {
            budget
                .at(CompileSite::ParserStorage)
                .reserve(1, mem::size_of::<ir::Node>())?;
        }
        Ok((Box::new(self.node), height))
    }

    fn alt(left: Self, right: Self, budget: Option<&CompileBudget>) -> Result<Self, CompileError> {
        let height = Self::parent_height(left.height.max(right.height), budget)?;
        if let Some(budget) = budget {
            budget
                .at(CompileSite::ParserStorage)
                .reserve(2, 2 * mem::size_of::<ir::Node>())?;
        }
        Ok(Self {
            node: ir::Node::Alt(Box::new(left.node), Box::new(right.node)),
            height,
        })
    }
}

fn make_cat(
    mut nodes: CompileVec<ParsedNode>,
    budget: Option<&CompileBudget>,
) -> Result<ParsedNode, CompileError> {
    match nodes.len() {
        0 => ParsedNode::leaf(ir::Node::Empty, budget),
        1 => Ok(nodes.pop(budget)?.unwrap()),
        _ => {
            if let Some(budget) = budget {
                budget.reserve(nodes.len(), 0)?;
            }
            let height = if budget.is_some() {
                ParsedNode::parent_height(nodes.iter().map(|n| n.height).max().unwrap(), budget)?
            } else {
                0
            };
            let mut children = Vec::new();
            if let Some(budget) = budget {
                let bytes = nodes
                    .len()
                    .checked_mul(mem::size_of::<ir::Node>())
                    .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
                budget.reserve(nodes.len(), bytes)?;
                children
                    .try_reserve_exact(nodes.len())
                    .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
            } else {
                children.reserve_exact(nodes.len());
            }
            children.extend(nodes.into_iter().map(|n| n.node));
            Ok(ParsedNode {
                node: ir::Node::Cat(children),
                height,
            })
        }
    }
}

fn make_alt(
    nodes: CompileVec<ParsedNode>,
    budget: Option<&CompileBudget>,
) -> Result<ParsedNode, CompileError> {
    let mut mright = None;
    for node in nodes.into_iter().rev() {
        mright = Some(match mright {
            None => node,
            Some(right) => ParsedNode::alt(node, right, budget)?,
        });
    }
    match mright {
        Some(node) => Ok(node),
        None => ParsedNode::leaf(ir::Node::Empty, budget),
    }
}

/// \return a CodePointSet for a given character escape (positive or negative).
/// See ES9 21.2.2.12.
/// Returns the positive (non-inverted) code point set for a character class.
fn codepoints_from_class_positive(
    ct: CharacterClassType,
    budget: Option<&CompileBudget>,
) -> Result<CodePointSet, CompileError> {
    let intervals: &[Interval] = match ct {
        CharacterClassType::Digits => &charclasses::DIGITS,
        CharacterClassType::Words => &charclasses::WORD_CHARS,
        CharacterClassType::Spaces => &charclasses::WHITESPACE,
    };
    if let Some(budget) = budget {
        budget
            .at(CompileSite::CodePointSet)
            .reserve(intervals.len(), 0)?;
        budget.reserve(intervals.len(), 0)?;
    }
    let mut cps = CodePointSet::from_sorted_disjoint_intervals(copy_slice(
        intervals,
        budget,
        CompileSite::CodePointSet,
    )?);
    if matches!(ct, CharacterClassType::Spaces) {
        for &iv in charclasses::LINE_TERMINATOR.iter() {
            cps.try_add(iv, budget)?;
        }
    }
    Ok(cps)
}

fn codepoints_from_class(
    ct: CharacterClassType,
    positive: bool,
    budget: Option<&CompileBudget>,
) -> Result<CodePointSet, CompileError> {
    let cps = codepoints_from_class_positive(ct, budget)?;
    if positive {
        Ok(cps)
    } else {
        cps.try_inverted(budget, CompileSite::CodePointSet)
    }
}

fn make_bracket_class(
    ct: CharacterClassType,
    positive: bool,
    icase: bool,
    budget: Option<&CompileBudget>,
) -> Result<ir::Node, CompileError> {
    let mut cps = codepoints_from_class_positive(ct, budget)?;
    if icase {
        cps = unicode::try_add_icase_code_points(cps, budget)?;
    }
    if !positive {
        cps = cps.try_inverted(budget, CompileSite::CodePointSet)?;
    }
    Ok(ir::Node::Bracket(BracketContents { invert: false, cps }))
}

fn add_class_atom(
    bc: &mut BracketContents,
    atom: ClassAtom,
    budget: Option<&CompileBudget>,
) -> Result<(), CompileError> {
    match atom {
        ClassAtom::CodePoint(c) => bc.cps.try_add_one(c, budget)?,
        ClassAtom::CharacterClass {
            class_type,
            positive,
        } => bc
            .cps
            .try_add_set(codepoints_from_class(class_type, positive, budget)?, budget)?,
        ClassAtom::Range { iv, negate } => {
            let iv = if negate {
                iv.try_inverted(budget, CompileSite::CodePointSet)?
            } else {
                iv
            };
            bc.cps.try_add_set(iv, budget)?;
        }
    }
    Ok(())
}

struct LookaroundParams {
    negate: bool,
    backwards: bool,
}

enum NameTable<V> {
    #[allow(
        clippy::disallowed_types,
        reason = "Only the unlimited compiler uses this map; bounded names use charged storage."
    )]
    Unlimited(HashMap<String, V>),
    Bounded(CompileVec<(String, V)>),
}

impl<V> NameTable<V> {
    #[allow(
        clippy::disallowed_types,
        reason = "Preserve the unlimited compiler map without changing bounded storage."
    )]
    fn new(budget: Option<&CompileBudget>) -> Self {
        if budget.is_some() {
            Self::Bounded(CompileVec::new())
        } else {
            Self::Unlimited(HashMap::new())
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Unlimited(values) => values.is_empty(),
            Self::Bounded(values) => values.is_empty(),
        }
    }

    fn get(&self, key: &str, budget: Option<&CompileBudget>) -> Result<Option<&V>, CompileError> {
        match self {
            Self::Unlimited(values) => Ok(values.get(key)),
            Self::Bounded(values) => {
                for (name, value) in values.iter() {
                    if let Some(budget) = budget {
                        budget.reserve(name.len().max(key.len()), 0)?;
                    }
                    if name == key {
                        return Ok(Some(value));
                    }
                }
                Ok(None)
            }
        }
    }

    fn for_each_value(
        &self,
        mut visit: impl FnMut(&V) -> Result<(), CompileError>,
    ) -> Result<(), CompileError> {
        match self {
            Self::Unlimited(values) => {
                for value in values.values() {
                    visit(value)?;
                }
            }
            Self::Bounded(values) => {
                for (_, value) in values.iter() {
                    visit(value)?;
                }
            }
        }
        Ok(())
    }
}

impl<V: Default> NameTable<V> {
    fn get_or_insert(
        &mut self,
        key: String,
        budget: Option<&CompileBudget>,
    ) -> Result<&mut V, CompileError> {
        match self {
            Self::Unlimited(values) => Ok(values.entry(key).or_default()),
            Self::Bounded(values) => {
                let mut found = None;
                for (index, (name, _)) in values.iter().enumerate() {
                    if let Some(budget) = budget {
                        budget.reserve(name.len().max(key.len()), 0)?;
                    }
                    if name == &key {
                        found = Some(index);
                        break;
                    }
                }
                if let Some(index) = found {
                    return Ok(&mut values[index].1);
                }
                values.push((key, V::default()), budget)?;
                Ok(&mut values.last_mut().unwrap().1)
            }
        }
    }
}

/// Represents an alternative path in a regex pattern.
/// For example, in `/(?<a>x)|(?<a>y)/`, the two occurrences of 'a' are in different
/// alternative paths (separated by |), so they don't conflict.
/// Each element in the vector is (depth, alternative_index) where:
/// - depth: parenthesis nesting level (0 = top level)
/// - alternative_index: which alternative at that depth (0 = first, 1 = second after |, etc.)
#[derive(Debug, Clone, PartialEq, Eq)]
struct AlternativePath {
    /// Vector of (depth, alternative_index) pairs representing the path through alternatives
    segments: Vec<(usize, usize)>,
}

impl AlternativePath {
    /// Check if two alternative paths conflict (i.e., are in the same alternative branch).
    /// Two paths conflict if they share the same alternative indices at all common depth levels.
    /// Example:
    ///   - [(0, 0)] and [(0, 0), (1, 0)] conflict (second is nested within first)
    ///   - [(0, 0)] and [(0, 1)] don't conflict (different alternatives at depth 0)
    fn conflicts_with(
        &self,
        other: &AlternativePath,
        budget: Option<&CompileBudget>,
    ) -> Result<bool, CompileError> {
        let min_len = self.segments.len().min(other.segments.len());
        if let Some(budget) = budget {
            budget.at(CompileSite::ParserCursor).reserve(min_len, 0)?;
        }
        Ok(self.segments[..min_len] == other.segments[..min_len])
    }
}

/// Represents the state used to parse a regex.
struct Parser<'a, I>
where
    I: Iterator<Item = u32>,
{
    budget: Option<&'a CompileBudget>,

    /// The remaining input.
    input: Peekable<I>,

    /// Flags used.
    flags: api::Flags,

    /// Number of loops.
    loop_count: u32,

    /// Number of capturing groups.
    group_count: CaptureGroupID,

    /// Maximum number of capturing groups.
    group_count_max: u32,

    /// A map each from capture group name to corresponding group indices in order.
    /// Note duplicate names may appear in distinct alternations, per the TC39 proposal.
    /// See <https://github.com/tc39/proposal-duplicate-named-capturing-groups>
    named_group_indices: NameTable<CompileVec<u32>>,

    /// Whether a lookbehind was encountered.
    has_lookbehind: bool,

    /// Current recursion nesting depth.
    depth: u32,
}

impl<I> Parser<'_, I>
where
    I: Iterator<Item = u32> + Clone,
{
    fn error<T>(&self, text: &str) -> Result<T, CompileError> {
        syntax_error(format_args!("{text}"), self.budget)
    }

    fn error_fmt<T>(&self, args: fmt::Arguments<'_>) -> Result<T, CompileError> {
        syntax_error(args, self.budget)
    }

    fn leaf(&self, node: ir::Node) -> Result<ParsedNode, CompileError> {
        ParsedNode::leaf(node, self.budget)
    }

    fn scan_work(&self, work: usize) -> Result<(), CompileError> {
        self.budget.map_or(Ok(()), |budget| {
            budget.at(CompileSite::ParserCursor).reserve(work, 0)
        })
    }

    fn read_work(&self) -> Result<(), CompileError> {
        self.scan_work(1)
    }

    fn cursor_next(&self, cursor: &mut Peekable<I>) -> Result<Option<u32>, CompileError> {
        self.read_work()?;
        Ok(cursor.next())
    }

    fn property_set(&self, intervals: &[Interval]) -> Result<CodePointSet, CompileError> {
        let Some(budget) = self.budget else {
            return Ok(CodePointSet::from_sorted_disjoint_intervals(
                intervals.to_vec(),
            ));
        };
        budget.at(CompileSite::PropertyCopy).check()?;
        let bytes = intervals
            .len()
            .checked_mul(mem::size_of::<Interval>())
            .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
        // Include the copy and the constructor's debug invariant scans.
        let work = intervals
            .len()
            .checked_mul(4)
            .ok_or_else(|| budget.refuse(CompileLimit::Work))?;
        budget.reserve(work, bytes)?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(intervals.len())
            .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
        owned.extend_from_slice(intervals);
        Ok(CodePointSet::from_sorted_disjoint_intervals(owned))
    }

    /// Consume a character, returning it.
    fn consume<C: Into<u32>>(&mut self, c: C) -> Result<u32, CompileError> {
        self.read_work()?;
        let nc = self.input.next();
        core::debug_assert!(nc == Some(c.into()), "char was not next");
        Ok(nc.unwrap())
    }

    /// If our contents begin with the char c, consume it from our contents
    /// and return true. Otherwise return false.
    fn try_consume<C: Into<u32>>(&mut self, c: C) -> Result<bool, CompileError> {
        self.read_work()?;
        Ok(self.input.next_if_eq(&c.into()).is_some())
    }

    /// If our contents begin with the string \p s, consume it from our contents
    /// and return true. Otherwise return false.
    fn try_consume_str(&mut self, s: &str) -> Result<bool, CompileError> {
        let mut cursor = self.input.clone();
        for c1 in s.chars() {
            if self.cursor_next(&mut cursor)? != Some(c1 as u32) {
                return Ok(false);
            }
        }
        self.input = cursor;
        Ok(true)
    }

    /// Build a node matching a code point `c`, optionally with case-insensitivity.
    fn char_node(&self, c: u32) -> Result<ParsedNode, CompileError> {
        if !self.flags.icase {
            return self.leaf(ir::Node::Char { c });
        }
        let class =
            unicode::try_expand_code_point(c, self.flags.icase, self.flags.unicode, self.budget)?;
        self.leaf(match class.len() {
            1 => ir::Node::Char { c: class[0] },
            2..=MAX_CHAR_SET_LENGTH => ir::Node::CharSet(class),
            _ => panic!("Unicode case fold exceeded maximum expansion"),
        })
    }

    /// Peek at the next character.
    fn peek(&mut self) -> Result<Option<u32>, CompileError> {
        self.read_work()?;
        Ok(self.input.peek().copied())
    }

    /// \return the next character.
    fn next(&mut self) -> Result<Option<u32>, CompileError> {
        self.read_work()?;
        Ok(self.input.next())
    }

    fn try_parse(&mut self) -> Result<ir::Regex, CompileError> {
        self.parse_capture_groups()?;

        // Parse a catenation. If we consume everything, it's success. If there's
        // something left, it's an error (for example, an excess closing paren).
        let body = self.consume_disjunction()?;
        match self.peek()? {
            Some(c) if c == ')' as u32 => self.error("Unbalanced parenthesis"),
            Some(c) => match char::from_u32(c) {
                Some(ch) => self.error_fmt(format_args!("Unexpected char: {ch}")),
                None => self.error_fmt(format_args!("Unexpected char: \\u{c:04X}")),
            },
            None => self.finalize(ir::Regex {
                node: make_cat(
                    node_list([body, self.leaf(ir::Node::Goal)?], self.budget)?,
                    self.budget,
                )?
                .node,
                flags: self.flags,
            }),
        }
    }

    /// ES6 21.2.2.3 Disjunction.
    fn consume_disjunction(&mut self) -> Result<ParsedNode, CompileError> {
        self.depth += 1;
        if self.depth > MAX_NESTING_DEPTH {
            return self.error("Regular expression is too deeply nested");
        }
        if let Some(budget) = self.budget {
            if self.depth as usize > budget.max_depth() {
                return Err(budget
                    .at(CompileSite::ParserHeight)
                    .refuse(CompileLimit::Depth));
            }
        }
        let mut terms = CompileVec::new();
        terms.push(self.consume_term()?, self.budget)?;
        while self.try_consume('|')? {
            terms.push(self.consume_term()?, self.budget)?
        }
        self.depth -= 1;
        make_alt(terms, self.budget)
    }

    /// ES6 21.2.2.5 Term.
    fn consume_term(&mut self) -> Result<ParsedNode, CompileError> {
        let mut result: CompileVec<ParsedNode> = CompileVec::new();
        loop {
            let start_group = self.group_count;
            let mut start_offset = result.len();
            let Some(c) = self.peek()? else {
                return make_cat(result, self.budget);
            };
            if matches!(to_char_sat(c), ')' | '|') {
                break;
            }
            let quantifier_allowed = if to_char_sat(c) == '(' {
                let (node, allowed) = self.consume_group()?;
                result.push(node, self.budget)?;
                if !self.try_consume(')')? {
                    return self.error("Unbalanced parenthesis");
                }
                allowed
            } else {
                self.consume_term_atom(c, &mut result, &mut start_offset)?
            };
            self.consume_term_quantifier(
                &mut result,
                start_offset,
                start_group,
                quantifier_allowed,
            )?;
        }
        make_cat(result, self.budget)
    }

    // Keep atom and quantifier temporaries out of recursive group frames.
    fn consume_term_atom(
        &mut self,
        c: u32,
        result: &mut CompileVec<ParsedNode>,
        start_offset: &mut usize,
    ) -> Result<bool, CompileError> {
        let mut quantifier_allowed = true;
        match to_char_sat(c) {
            // Term :: Assertion :: ^
            '^' => {
                self.consume('^')?;
                result.push(
                    self.leaf(ir::Node::Anchor {
                        anchor_type: ir::AnchorType::StartOfLine,
                        multiline: self.flags.multiline,
                    })?,
                    self.budget,
                )?;
                quantifier_allowed = false;
            }
            // Term :: Assertion :: $
            '$' => {
                self.consume('$')?;
                result.push(
                    self.leaf(ir::Node::Anchor {
                        anchor_type: ir::AnchorType::EndOfLine,
                        multiline: self.flags.multiline,
                    })?,
                    self.budget,
                )?;
                quantifier_allowed = false;
            }

            '\\' => {
                self.consume('\\')?;
                let Some(c) = self.peek()? else {
                    return self.error("Incomplete escape");
                };
                match to_char_sat(c) {
                    // Term :: Assertion :: \b
                    'b' => {
                        self.consume('b')?;
                        result.push(
                            self.leaf(ir::Node::WordBoundary {
                                invert: false,
                                unicode_icase: self.flags.unicode && self.flags.icase,
                            })?,
                            self.budget,
                        )?;
                    }
                    // Term :: Assertion :: \B
                    'B' => {
                        self.consume('B')?;
                        result.push(
                            self.leaf(ir::Node::WordBoundary {
                                invert: true,
                                unicode_icase: self.flags.unicode && self.flags.icase,
                            })?,
                            self.budget,
                        )?;
                    }
                    // Term :: Atom :: \ AtomEscape :: CharacterEscape :: c AsciiLetter
                    // Term :: ExtendedAtom :: \ [lookahead = c]
                    'c' if !self.flags.unicode => {
                        self.consume('c')?;
                        if self
                            .peek()?
                            .and_then(char::from_u32)
                            .map(|c| c.is_ascii_alphabetic())
                            == Some(true)
                        {
                            let cp = self.next()?.expect("char was not next") % 32;
                            result.push(self.char_node(cp)?, self.budget)?;
                        } else {
                            *start_offset += 1;
                            result.push(self.char_node(u32::from('\\'))?, self.budget)?;
                            result.push(self.char_node(u32::from('c'))?, self.budget)?;
                        }
                    }
                    // Term :: Atom :: \ AtomEscape
                    _ => {
                        result.push(self.consume_atom_escape()?, self.budget)?;
                    }
                }
            }

            // Term :: Atom :: .
            '.' => {
                self.consume('.')?;
                result.push(
                    self.leaf(if self.flags.dot_all {
                        ir::Node::MatchAny
                    } else {
                        ir::Node::MatchAnyExceptLineTerminator
                    })?,
                    self.budget,
                )?;
            }

            // CharacterClass :: ClassContents :: ClassSetExpression
            '[' if self.flags.unicode_sets => {
                self.consume('[')?;
                let negate_set = self.try_consume('^')?;
                result.push(
                    self.consume_class_set_expression()?.node(
                        self.flags.icase,
                        negate_set,
                        self.budget,
                    )?,
                    self.budget,
                )?;
            }

            '[' => {
                result.push(self.consume_bracket()?, self.budget)?;
            }

            // Term :: ExtendedAtom :: InvalidBracedQuantifier
            '{' if !self.flags.unicode => {
                if self.try_consume_braced_quantifier()?.is_some() {
                    return self.error("Invalid braced quantifier");
                }

                // Term :: ExtendedAtom :: ExtendedPatternCharacter
                let cp = self.consume(c)?;
                result.push(self.char_node(cp)?, self.budget)?
            }

            // Term :: Atom :: PatternCharacter :: SourceCharacter but not ^ $ \ . * + ? ( ) [ ] { } |
            '*' | '+' | '?' | ']' | '{' | '}' if self.flags.unicode => {
                return self.error("Invalid atom character");
            }

            // Term :: ExtendedAtom :: SourceCharacter but not ^ $ \ . * + ? ( ) [ |
            '*' | '+' | '?' => {
                return self.error("Invalid atom character");
            }

            // Term :: Atom :: PatternCharacter
            // Term :: ExtendedAtom :: ExtendedPatternCharacter
            _ => {
                self.consume(c)?;
                result.push(self.char_node(c)?, self.budget)?
            }
        }
        Ok(quantifier_allowed)
    }

    fn consume_group(&mut self) -> Result<(ParsedNode, bool), CompileError> {
        if self.try_consume_str("(?=")? {
            let allowed = !self.flags.unicode;
            return Ok((
                self.consume_lookaround_assertion(LookaroundParams {
                    negate: false,
                    backwards: false,
                })?,
                allowed,
            ));
        }
        if self.try_consume_str("(?!")? {
            let allowed = !self.flags.unicode;
            return Ok((
                self.consume_lookaround_assertion(LookaroundParams {
                    negate: true,
                    backwards: false,
                })?,
                allowed,
            ));
        }
        if self.try_consume_str("(?<=")? {
            self.has_lookbehind = true;
            return Ok((
                self.consume_lookaround_assertion(LookaroundParams {
                    negate: false,
                    backwards: true,
                })?,
                false,
            ));
        }
        if self.try_consume_str("(?<!")? {
            self.has_lookbehind = true;
            return Ok((
                self.consume_lookaround_assertion(LookaroundParams {
                    negate: true,
                    backwards: true,
                })?,
                false,
            ));
        }
        if self.try_consume_str("(?:")? {
            return Ok((self.consume_disjunction()?, true));
        }
        if let Some(group) = self.try_consume_modifier_group()? {
            return Ok((group, true));
        }
        Ok((self.consume_capture_group()?, true))
    }

    fn consume_capture_group(&mut self) -> Result<ParsedNode, CompileError> {
        self.consume('(')?;
        let group = self.group_count;
        if self.group_count as usize >= MAX_CAPTURE_GROUPS {
            return self.error("Capture group count limit exceeded");
        }
        self.group_count += 1;
        let mut group_name = None;
        if self.try_consume_str("?")? {
            group_name = self.try_consume_named_capture_group_name()?;
            if group_name.is_none() {
                return self.error("Invalid token at named capture group identifier");
            }
        }
        let (contents, height) = self.consume_disjunction()?.boxed(self.budget)?;
        Ok(ParsedNode {
            node: ir::Node::CaptureGroup {
                id: group,
                contents,
                name: group_name,
            },
            height,
        })
    }

    fn consume_term_quantifier(
        &mut self,
        result: &mut CompileVec<ParsedNode>,
        start_offset: usize,
        start_group: CaptureGroupID,
        quantifier_allowed: bool,
    ) -> Result<(), CompileError> {
        // We just parsed a term; try parsing a quantifier.
        if let Some(quant) = self.try_consume_quantifier()? {
            if !quantifier_allowed {
                return self.error("Quantifier not allowed here");
            }
            // Validate the quantifier.
            // Note we don't want to do this as part of parsing the quantiifer in some cases
            // an incomplete quantifier is not recognized as a quantifier, e.g. `/{3/` is
            // valid.
            if matches!(quant.max, Some(max) if quant.min > max) {
                return self.error("Invalid quantifier");
            }
            let quantifee = result.split_off(start_offset, self.budget)?;
            if self.loop_count as usize >= MAX_LOOPS {
                return self.error("Loop count limit exceeded");
            }
            self.loop_count += 1;
            let (loopee, height) = make_cat(quantifee, self.budget)?.boxed(self.budget)?;
            result.push(
                ParsedNode {
                    node: ir::Node::Loop {
                        loopee,
                        quant,
                        enclosed_groups: start_group..self.group_count,
                    },
                    height,
                },
                self.budget,
            )?;
        }
        Ok(())
    }

    fn try_consume_modifier_group(&mut self) -> Result<Option<ParsedNode>, CompileError> {
        let mut cursor = self.input.clone();
        if self.cursor_next(&mut cursor)? != Some('(' as u32) {
            return Ok(None);
        }
        if self.cursor_next(&mut cursor)? != Some('?' as u32) {
            return Ok(None);
        }
        let Some(mut current) = self.cursor_next(&mut cursor)? else {
            return Ok(None);
        };

        if to_char_sat(current) == '<' {
            return Ok(None);
        }

        let mut seen_hyphen = false;
        let mut saw_flag = false;
        let mut icase_override: Option<bool> = None;
        let mut multiline_override: Option<bool> = None;
        let mut dot_all_override: Option<bool> = None;

        loop {
            let ch = to_char_sat(current);
            match ch {
                'i' | 'm' | 's' => {
                    let value = !seen_hyphen;
                    let target = match ch {
                        'i' => &mut icase_override,
                        'm' => &mut multiline_override,
                        's' => &mut dot_all_override,
                        _ => unreachable!(),
                    };
                    if target.is_some() {
                        return self.error("Invalid group modifier");
                    }
                    *target = Some(value);
                    saw_flag = true;
                }
                '-' => {
                    if seen_hyphen {
                        return self.error("Invalid group modifier");
                    }
                    seen_hyphen = true;
                }
                ':' => {
                    if !saw_flag {
                        return self.error("Invalid group modifier");
                    }
                    self.input = cursor;
                    let mut new_flags = self.flags;
                    if let Some(value) = icase_override {
                        new_flags.icase = value;
                    }
                    if let Some(value) = multiline_override {
                        new_flags.multiline = value;
                    }
                    if let Some(value) = dot_all_override {
                        new_flags.dot_all = value;
                    }
                    let saved_flags = self.flags;
                    self.flags = new_flags;
                    let contents = match self.consume_disjunction() {
                        Ok(node) => node,
                        Err(err) => {
                            self.flags = saved_flags;
                            return Err(err);
                        }
                    };
                    self.flags = saved_flags;
                    return Ok(Some(contents));
                }
                _ => {
                    return self.error("Invalid group modifier");
                }
            }

            current = match self.cursor_next(&mut cursor)? {
                Some(next) => next,
                None => return self.error("Invalid group modifier"),
            };
        }
    }

    /// ES6 21.2.2.13 CharacterClass.
    fn consume_bracket(&mut self) -> Result<ParsedNode, CompileError> {
        self.consume('[')?;
        let invert = self.try_consume('^')?;
        let mut result = BracketContents {
            invert,
            cps: CodePointSet::default(),
        };

        loop {
            match self.peek()?.map(to_char_sat) {
                None => {
                    return self.error("Unbalanced bracket");
                }
                Some(']') => {
                    self.consume(']')?;
                    if self.flags.icase {
                        result.cps = unicode::try_add_icase_code_points(result.cps, self.budget)?;
                    }
                    return self.leaf(ir::Node::Bracket(result));
                }
                _ => {}
            }

            // Parse a code point or character class.
            let Some(first) = self.try_consume_bracket_class_atom()? else {
                continue;
            };

            // Check for a dash; we may have a range.
            if !self.try_consume('-')? {
                add_class_atom(&mut result, first, self.budget)?;
                continue;
            }

            let Some(second) = self.try_consume_bracket_class_atom()? else {
                // No second atom. For example: [a-].
                add_class_atom(&mut result, first, self.budget)?;
                add_class_atom(
                    &mut result,
                    ClassAtom::CodePoint(u32::from('-')),
                    self.budget,
                )?;
                continue;
            };

            // Ranges must also be in order: z-a is invalid.
            // ES6 21.2.2.15.1 "If i > j, throw a SyntaxError exception"
            if let (ClassAtom::CodePoint(c1), ClassAtom::CodePoint(c2)) = (&first, &second) {
                if c1 > c2 {
                    return self.error(
                        "Range values reversed, start char code is greater than end char code.",
                    );
                }
                result.cps.try_add(
                    Interval {
                        first: *c1,
                        last: *c2,
                    },
                    self.budget,
                )?;

                continue;
            }

            if self.flags.unicode {
                return self.error("Invalid character range");
            }

            // If it does not match a range treat as any match single characters.
            add_class_atom(&mut result, first, self.budget)?;
            add_class_atom(
                &mut result,
                ClassAtom::CodePoint(u32::from('-')),
                self.budget,
            )?;
            add_class_atom(&mut result, second, self.budget)?;
        }
    }

    fn try_consume_bracket_class_atom(&mut self) -> Result<Option<ClassAtom>, CompileError> {
        let c = self.peek()?;
        if c.is_none() {
            return Ok(None);
        }
        let c = c.unwrap();
        match to_char_sat(c) {
            // End of bracket.
            ']' => Ok(None),

            // ClassEscape
            '\\' => {
                self.consume('\\')?;
                let ec = if let Some(ec) = self.peek()? {
                    ec
                } else {
                    return self.error("Unterminated escape");
                };
                match to_char_sat(ec) {
                    // ClassEscape :: b
                    'b' => {
                        self.consume('b')?;
                        Ok(Some(ClassAtom::CodePoint(u32::from('\x08'))))
                    }
                    // ClassEscape :: [+UnicodeMode] -
                    '-' if self.flags.unicode => {
                        self.consume('-')?;
                        Ok(Some(ClassAtom::CodePoint(u32::from('-'))))
                    }
                    'c' if !self.flags.unicode => {
                        let input = self.input.clone();
                        self.consume('c')?;
                        match self.peek()?.map(to_char_sat) {
                            // ClassEscape :: [~UnicodeMode] c ClassControlLetter
                            Some('0'..='9' | '_') => {
                                let next = self.next()?.expect("char was not next");
                                Ok(Some(ClassAtom::CodePoint(next & 0x1F)))
                            }
                            // CharacterEscape :: c AsciiLetter
                            Some('a'..='z' | 'A'..='Z') => {
                                let next = self.next()?.expect("char was not next");
                                Ok(Some(ClassAtom::CodePoint(next % 32)))
                            }
                            // ClassAtomNoDash :: \ [lookahead = c]
                            _ => {
                                self.input = input;
                                Ok(Some(ClassAtom::CodePoint(u32::from('\\'))))
                            }
                        }
                    }
                    // ClassEscape :: CharacterClassEscape :: d
                    'd' => {
                        self.consume('d')?;
                        Ok(Some(ClassAtom::CharacterClass {
                            class_type: CharacterClassType::Digits,
                            positive: true,
                        }))
                    }
                    // ClassEscape :: CharacterClassEscape :: D
                    'D' => {
                        self.consume('D')?;
                        Ok(Some(ClassAtom::CharacterClass {
                            class_type: CharacterClassType::Digits,
                            positive: false,
                        }))
                    }
                    // ClassEscape :: CharacterClassEscape :: s
                    's' => {
                        self.consume('s')?;
                        Ok(Some(ClassAtom::CharacterClass {
                            class_type: CharacterClassType::Spaces,
                            positive: true,
                        }))
                    }
                    // ClassEscape :: CharacterClassEscape :: S
                    'S' => {
                        self.consume('S')?;
                        Ok(Some(ClassAtom::CharacterClass {
                            class_type: CharacterClassType::Spaces,
                            positive: false,
                        }))
                    }
                    // ClassEscape :: CharacterClassEscape :: w
                    'w' => {
                        self.consume('w')?;
                        Ok(Some(ClassAtom::CharacterClass {
                            class_type: CharacterClassType::Words,
                            positive: true,
                        }))
                    }
                    // ClassEscape :: CharacterClassEscape :: W
                    'W' => {
                        self.consume('W')?;
                        Ok(Some(ClassAtom::CharacterClass {
                            class_type: CharacterClassType::Words,
                            positive: false,
                        }))
                    }
                    // ClassEscape :: CharacterClassEscape :: [+UnicodeMode] p{ UnicodePropertyValueExpression }
                    // ClassEscape :: CharacterClassEscape :: [+UnicodeMode] P{ UnicodePropertyValueExpression }
                    'p' | 'P' if self.flags.unicode => {
                        self.consume(ec)?;
                        let negate = ec == 'P' as u32;
                        match self.try_consume_unicode_property_escape()? {
                            PropertyEscapeKind::CharacterClass(s) => Ok(Some(ClassAtom::Range {
                                iv: self.property_set(s)?,
                                negate,
                            })),
                            PropertyEscapeKind::StringSet(_) => {
                                self.error("Invalid property escape")
                            }
                        }
                    }
                    // ClassEscape :: CharacterEscape
                    _ => {
                        let cc = self.consume_character_escape()?;
                        Ok(Some(ClassAtom::CodePoint(cc)))
                    }
                }
            }

            _ => Ok(Some(ClassAtom::CodePoint(self.consume(c)?))),
        }
    }

    // CharacterClass :: ClassContents :: ClassSetExpression
    fn consume_class_set_expression(&mut self) -> Result<ClassSet, CompileError> {
        let (result, op) = self.consume_class_set_start()?;
        match op {
            ClassSetOperator::Complete => Ok(result),
            ClassSetOperator::Union => self.consume_class_set_union(result),
            ClassSetOperator::Intersection => self.consume_class_set_intersection(result),
            ClassSetOperator::Subtraction => self.consume_class_set_subtraction(result),
        }
    }

    fn consume_class_set_start(&mut self) -> Result<(ClassSet, ClassSetOperator), CompileError> {
        let mut result = ClassSet::new();

        let first = match self.peek()? {
            Some(0x5D /* ] */) => {
                self.consume(']')?;
                return Ok((result, ClassSetOperator::Complete));
            }
            Some(_) => self.consume_class_set_operand()?,
            None => {
                return self.error("Unbalanced class set bracket");
            }
        };

        let op = match self.peek()? {
            Some(0x5D /* ] */) => {
                self.consume(']')?;
                result.union_operand(first, self.budget)?;
                return Ok((result, ClassSetOperator::Complete));
            }
            Some(0x26 /* & */) => {
                self.consume('&')?;
                if self.peek()? == Some(0x26 /* & */) {
                    self.consume('&')?;
                    result.union_operand(first.try_clone(self.budget)?, self.budget)?;
                    ClassSetOperator::Intersection
                } else {
                    result.codepoints.try_add_one(0x26 /* & */, self.budget)?;
                    result.union_operand(first, self.budget)?;
                    ClassSetOperator::Union
                }
            }
            Some(0x2D /* - */) => {
                self.consume('-')?;
                if self.peek()? == Some(0x2D /* - */) {
                    self.consume('-')?;
                    result.union_operand(first.try_clone(self.budget)?, self.budget)?;
                    ClassSetOperator::Subtraction
                } else {
                    match first {
                        ClassSetOperand::ClassSetCharacter(first) => {
                            let ClassSetOperand::ClassSetCharacter(last) =
                                self.consume_class_set_operand()?
                            else {
                                return self.error("Invalid class set range");
                            };
                            if first > last {
                                return self.error("Invalid class set range");
                            }
                            result
                                .codepoints
                                .try_add(Interval { first, last }, self.budget)?;
                        }
                        _ => {
                            return self.error("Invalid class set range");
                        }
                    };
                    ClassSetOperator::Union
                }
            }
            Some(_) => {
                result.union_operand(first.try_clone(self.budget)?, self.budget)?;
                ClassSetOperator::Union
            }
            None => {
                return self.error("Unbalanced class set bracket");
            }
        };

        Ok((result, op))
    }

    fn consume_class_set_union(&mut self, mut result: ClassSet) -> Result<ClassSet, CompileError> {
        loop {
            let operand = match self.peek()? {
                Some(0x5D /* ] */) => {
                    self.consume(']')?;
                    return Ok(result);
                }
                Some(_) => self.consume_class_set_operand()?,
                None => return self.error("Unbalanced class set bracket"),
            };
            if self.peek()? == Some(0x2D /* - */) {
                self.consume('-')?;
                match operand {
                    ClassSetOperand::ClassSetCharacter(first) => {
                        let ClassSetOperand::ClassSetCharacter(last) =
                            self.consume_class_set_operand()?
                        else {
                            return self.error("Invalid class set range");
                        };
                        if first > last {
                            return self.error("Invalid class set range");
                        }
                        result
                            .codepoints
                            .try_add(Interval { first, last }, self.budget)?;
                    }
                    _ => {
                        return self.error("Invalid class set range");
                    }
                };
            } else {
                result.union_operand(operand, self.budget)?;
            }
        }
    }

    fn consume_class_set_intersection(
        &mut self,
        mut result: ClassSet,
    ) -> Result<ClassSet, CompileError> {
        loop {
            let operand = self.consume_class_set_operand()?;
            result.intersect_operand(operand, self.budget)?;
            match self.next()? {
                Some(0x5D /* ] */) => return Ok(result),
                Some(0x26 /* & */) => {}
                Some(_) => {
                    return self.error("Unexpected character in class set intersection");
                }
                _ => return self.error("Unbalanced class set bracket"),
            }
            if self.next()? != Some(0x26 /* & */) {
                return self.error("Unbalanced class set bracket");
            }
        }
    }

    fn consume_class_set_subtraction(
        &mut self,
        mut result: ClassSet,
    ) -> Result<ClassSet, CompileError> {
        loop {
            let operand = self.consume_class_set_operand()?;
            result.subtract_operand(operand, self.budget)?;
            match self.next()? {
                Some(0x5D /* ] */) => return Ok(result),
                Some(0x2D /* - */) => {}
                Some(_) => {
                    return self.error("Unexpected character in class set subtraction");
                }
                _ => return self.error("Unbalanced class set bracket"),
            }
            if self.next()? != Some(0x2D /* - */) {
                return self.error("Unbalanced class set bracket");
            }
        }
    }

    fn consume_class_set_operand(&mut self) -> Result<ClassSetOperand, CompileError> {
        use ClassSetOperand::*;
        let Some(cp) = self.peek()? else {
            return self.error("Empty class set operand");
        };
        match cp {
            // ClassSetOperand :: NestedClass :: [ [lookahead ≠ ^] ClassContents[+UnicodeMode, +UnicodeSetsMode] ]
            // ClassSetOperand :: NestedClass :: [^ ClassContents[+UnicodeMode, +UnicodeSetsMode] ]
            0x5B /* [ */ => {
                self.depth += 1;
                if self.depth > MAX_NESTING_DEPTH {
                    return self.error("Regular expression is too deeply nested");
                }
        if let Some(budget) = self.budget {
            if self.depth as usize > budget.max_depth() {
                return Err(budget.at(CompileSite::ParserHeight).refuse(CompileLimit::Depth));
            }
        }
                self.consume('[')?;
                let negate_set = self.try_consume('^')?;
                let mut result = self.consume_class_set_expression()?;
                if negate_set {
                    result.prepare_negation(self.budget)?;
                    if self.flags.icase {
                        result.codepoints = unicode::try_add_icase_code_points(result.codepoints, self.budget)?;
                    }
                    result.codepoints = result.codepoints.try_inverted(self.budget, CompileSite::ClassSet)?;
                }
                self.depth -= 1;
                Ok(Class(result))
            }
            // ClassSetOperand :: NestedClass :: \ CharacterClassEscape
            // ClassSetOperand :: ClassStringDisjunction
            // ClassSetOperand :: ClassSetCharacter :: \...
            // ClassSetRange :: ClassSetCharacter :: \...
            0x5C /* \ */ => self.consume_class_set_escape(),
            // ClassSetOperand :: ClassSetCharacter
            // ClassSetRange :: ClassSetCharacter
            _ => Ok(ClassSetCharacter(self.consume_class_set_character()?)),
        }
    }

    fn consume_class_set_escape(&mut self) -> Result<ClassSetOperand, CompileError> {
        use ClassSetOperand::*;
        self.consume('\\')?;
        let Some(cp) = self.peek()? else {
            return self.error("Incomplete class set escape");
        };
        match cp {
            // ClassStringDisjunction  \q{ ClassStringDisjunctionContents }
            0x71 /* q */ => {
                self.consume('q')?;
                if !self.try_consume('{')? {
                    return self.error("Invalid class set escape: expected {");
                }
                let mut alternatives = CompileVec::new_at(CompileSite::ClassSet);
                let mut alternative = CompileVec::new_at(CompileSite::ClassSet);
                let mut may_contain_strings = false;
                loop {
                    match self.peek()? {
                        Some(0x7D /* } */) => {
                            self.consume('}')?;
                            may_contain_strings |= alternative.len() != 1;
                            alternatives.push(boxed_slice(alternative.into_vec(), self.budget, CompileSite::ClassSet)?, self.budget)?;
                            break;
                        }
                        Some(0x7C /* | */) => {
                            self.consume('|')?;
                            may_contain_strings |= alternative.len() != 1;
                            let alternative = boxed_slice(mem::take(&mut alternative).into_vec(), self.budget, CompileSite::ClassSet)?;
                            alternatives.push(alternative, self.budget)?;
                        }
                        Some(_) => {
                            alternative.push(self.consume_class_set_character()?, self.budget)?;
                        }
                        None => {
                            return self.error("Unbalanced class set string disjunction");
                        }
                    }
                }
                Ok(ClassStringDisjunction(ClassSetAlternativeStrings(alternatives), may_contain_strings))
            }
            // CharacterClassEscape :: d
            0x64 /* d */ => {
                self.consume('d')?;
                Ok(CharacterClassEscape(codepoints_from_class(CharacterClassType::Digits, true, self.budget)?))
            }
            // CharacterClassEscape :: D
            0x44 /* D */ => {
                self.consume('D')?;
                Ok(CharacterClassEscape(codepoints_from_class(CharacterClassType::Digits, false, self.budget)?))
            }
            // CharacterClassEscape :: s
            0x73 /* s */ => {
                self.consume('s')?;
                Ok(CharacterClassEscape(codepoints_from_class(CharacterClassType::Spaces, true, self.budget)?))
            }
            // CharacterClassEscape :: S
            0x53 /* S */ => {
                self.consume('S')?;
                Ok(CharacterClassEscape(codepoints_from_class(CharacterClassType::Spaces, false, self.budget)?))
            }
            // CharacterClassEscape :: w
            0x77 /* w */ => {
                self.consume('w')?;
                Ok(CharacterClassEscape(codepoints_from_class(CharacterClassType::Words, true, self.budget)?))
            }
            // CharacterClassEscape :: W
            0x57 /* W */ => {
                self.consume('W')?;
                Ok(CharacterClassEscape(codepoints_from_class(CharacterClassType::Words, false, self.budget)?))
            }
            // CharacterClassEscape :: [+UnicodeMode] p{ UnicodePropertyValueExpression }
            0x70 /* p */ => {
                self.consume('p')?;
                match self.try_consume_unicode_property_escape()? {
                    PropertyEscapeKind::CharacterClass(intervals) => {
                        Ok(CharacterClassEscape(self.property_set(intervals)?))
                    }
                    PropertyEscapeKind::StringSet(strings) => {
                        Ok(ClassStringDisjunction(ClassSetAlternativeStrings::from_slices(strings, self.budget)?, true))
                    }
                }
            }
            // CharacterClassEscape :: [+UnicodeMode] P{ UnicodePropertyValueExpression }
            0x50 /* P */ => {
                self.consume('P')?;
                match self.try_consume_unicode_property_escape()? {
                    PropertyEscapeKind::CharacterClass(s) => {
                        Ok(CharacterClassEscape(self.property_set(s)?.try_inverted(self.budget, CompileSite::ClassSet)?))
                    }
                    PropertyEscapeKind::StringSet(_) => self.error("Invalid character escape"),
                }
            }
            // ClassSetCharacter:: \b
            0x62 /* b */ => {
                Ok(ClassSetCharacter(self.consume(cp)?))
            }
            // ClassSetCharacter:: \ ClassSetReservedPunctuator
            _ if Self::is_class_set_reserved_punctuator(cp) => Ok(ClassSetCharacter(self.consume(cp)?)),
            // ClassSetCharacter:: \ CharacterEscape[+UnicodeMode]
            _ => Ok(ClassSetCharacter(self.consume_character_escape()?))
        }
    }

    // ClassSetCharacter
    fn consume_class_set_character(&mut self) -> Result<u32, CompileError> {
        let Some(cp) = self.next()? else {
            return self.error("Incomplete class set character");
        };
        match cp {
            0x5C /* \ */ => {
                let Some(cp) = self.peek()? else {
                    return self.error("Incomplete class set escape");
                };
                match cp {
                    // \b
                    0x62 /* b */ => {
                        Ok(self.consume(cp)?)
                    }
                    // \ ClassSetReservedPunctuator
                    _ if Self::is_class_set_reserved_punctuator(cp) => Ok(self.consume(cp)?),
                    // \ CharacterEscape[+UnicodeMode]
                    _ => Ok(self.consume_character_escape()?)
                }
            }
            // [lookahead ∉ ClassSetReservedDoublePunctuator] SourceCharacter but not ClassSetSyntaxCharacter
            0x28 /* ( */ | 0x29 /* ) */ | 0x7B /* { */ | 0x7D /* } */ | 0x2F /* / */
            | 0x2D /* - */ | 0x7C /* | */ => self.error("Invalid class set character"),
            _ => {
                if Self::is_class_set_reserved_double_punctuator(cp)
                    && let Some(cp) = self.peek()?
                        && Self::is_class_set_reserved_double_punctuator(cp) {
                            return self.error("Invalid class set character");
                        }
                Ok(cp)
            }
        }
    }

    // ClassSetReservedPunctuator
    fn is_class_set_reserved_punctuator(cp: u32) -> bool {
        match cp {
            0x26 /* & */ | 0x2D /* - */ | 0x21 /* ! */ | 0x23 /* # */ | 0x25 /* % */
            | 0x2C /* , */ | 0x3A /* : */ | 0x3B /* ; */ | 0x3C /* < */ | 0x3D /* = */
            | 0x3E /* > */ | 0x40 /* @ */ | 0x60 /* ` */ | 0x7E /* ~ */ => true,
            _ => false,
        }
    }

    fn is_class_set_reserved_double_punctuator(cp: u32) -> bool {
        match cp {
            0x26 /* & */ | 0x21 /* ! */ | 0x23 /* # */ | 0x24 /* $ */ | 0x25 /* % */
            | 0x2A /* * */ | 0x2B /* + */ | 0x2C /* , */ | 0x2E /* . */ | 0x3A /* : */
            | 0x3B /* ; */ | 0x3C /* < */ | 0x3D /* = */ | 0x3E /* > */ | 0x3F /* ? */
            | 0x40 /* @ */ | 0x5E /* ^ */ | 0x60 /* ` */ | 0x7E /* ~ */ => true,
            _ => false,
        }
    }

    fn try_consume_quantifier(&mut self) -> Result<Option<ir::Quantifier>, CompileError> {
        if let Some(mut quant) = self.try_consume_quantifier_prefix()? {
            quant.greedy = !self.try_consume('?')?;
            Ok(Some(quant))
        } else {
            Ok(None)
        }
    }

    fn try_consume_quantifier_prefix(&mut self) -> Result<Option<ir::Quantifier>, CompileError> {
        let nc = self.peek()?;
        if nc.is_none() {
            return Ok(None);
        }
        let c = nc.unwrap();
        match char::from_u32(c) {
            Some('+') => {
                self.consume('+')?;
                Ok(Some(ir::Quantifier {
                    min: 1,
                    max: None,
                    greedy: true,
                }))
            }
            Some('*') => {
                self.consume('*')?;
                Ok(Some(ir::Quantifier {
                    min: 0,
                    max: None,
                    greedy: true,
                }))
            }
            Some('?') => {
                self.consume('?')?;
                Ok(Some(ir::Quantifier {
                    min: 0,
                    max: Some(1),
                    greedy: true,
                }))
            }
            Some('{') => {
                if let Some(quantifier) = self.try_consume_braced_quantifier()? {
                    Ok(Some(quantifier))
                } else if self.flags.unicode {
                    // if there was a brace '{' that doesn't parse into a valid quantifier,
                    // it's not valid with the unicode flag
                    self.error("Invalid quantifier")
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn try_consume_braced_quantifier(&mut self) -> Result<Option<ir::Quantifier>, CompileError> {
        // if parsed input is actually invalid, keep the previous one for rollback
        let pre_input = self.input.clone();
        self.consume('{')?;
        let optmin = self.try_consume_decimal_integer_literal()?;
        let Some(optmin) = optmin else {
            // not a valid quantifier, rollback consumption
            self.input = pre_input;
            return Ok(None);
        };
        let mut quant = ir::Quantifier {
            min: optmin,
            max: Some(optmin),
            greedy: true,
        };
        if self.try_consume(',')? {
            let max = self.try_consume_decimal_integer_literal()?;
            // Either like {3,4} in which case we want to set the max;
            // or like {3,} in which case the max should be None to indicate unbounded.
            quant.max = max;
        } else {
            // Like {3}.
        }
        if !self.try_consume('}')? {
            // not a valid quantifier, rollback consumption
            self.input = pre_input;
            return Ok(None);
        }
        Ok(Some(quant))
    }

    /// ES6 11.8.3 DecimalIntegerLiteral.
    /// If the value would overflow, usize::MAX is returned.
    /// All decimal digits are consumed regardless.
    fn try_consume_decimal_integer_literal(&mut self) -> Result<Option<usize>, CompileError> {
        let mut result: usize = 0;
        let mut saw_digit = false;
        while let Some(c) = self.peek()? {
            if let Some(digit) = char::from_u32(c).and_then(|c| char::to_digit(c, 10)) {
                self.consume(c)?;
                saw_digit = true;
                result = result.saturating_mul(10);
                result = result.saturating_add(digit as usize);
            } else {
                break;
            }
        }
        Ok(if saw_digit { Some(result) } else { None })
    }

    fn consume_lookaround_assertion(
        &mut self,
        params: LookaroundParams,
    ) -> Result<ParsedNode, CompileError> {
        let start_group = self.group_count;
        let contents = self.consume_disjunction()?;
        let end_group = self.group_count;
        let (contents, height) = contents.boxed(self.budget)?;
        Ok(ParsedNode {
            node: ir::Node::LookaroundAssertion {
                negate: params.negate,
                backwards: params.backwards,
                start_group,
                end_group,
                contents,
            },
            height,
        })
    }

    fn consume_character_escape(&mut self) -> Result<u32, CompileError> {
        let c = self.next()?.expect("Should have a character");
        let ch = to_char_sat(c);
        match ch {
            // CharacterEscape :: ControlEscape :: f
            'f' => Ok(0xC),
            // CharacterEscape :: ControlEscape :: n
            'n' => Ok(0xA),
            // CharacterEscape :: ControlEscape :: r
            'r' => Ok(0xD),
            // CharacterEscape :: ControlEscape :: t
            't' => Ok(0x9),
            // CharacterEscape :: ControlEscape :: v
            'v' => Ok(0xB),
            // CharacterEscape :: c AsciiLetter
            'c' => {
                if let Some(nc) = self.next()?.and_then(char::from_u32)
                    && (nc.is_ascii_lowercase() || nc.is_ascii_uppercase())
                {
                    return Ok((nc as u32) % 32);
                }
                self.error("Invalid character escape")
            }
            // CharacterEscape :: 0 [lookahead ∉ DecimalDigit]
            '0' if self
                .peek()?
                .and_then(char::from_u32)
                .map(|c: char| c.is_ascii_digit())
                != Some(true) =>
            {
                Ok(0x0)
            }
            // CharacterEscape :: HexEscapeSequence :: x HexDigit HexDigit
            'x' => {
                let orig_input = self.input.clone();
                let hex_to_digit = |c: char| c.to_digit(16);
                let x1 = self.next()?.and_then(char::from_u32).and_then(hex_to_digit);
                let x2 = self.next()?.and_then(char::from_u32).and_then(hex_to_digit);
                match (x1, x2) {
                    (Some(x1), Some(x2)) => Ok(x1 * 16 + x2),
                    // CharacterEscape :: IdentityEscape :: SourceCharacterIdentityEscape
                    // Not a valid HexEscapeSequence. Restore the input.
                    _ if !self.flags.unicode => {
                        self.input = orig_input;
                        Ok(c)
                    }
                    _ => self.error("Invalid character escape"),
                }
            }
            // CharacterEscape :: RegExpUnicodeEscapeSequence
            'u' => {
                if let Some(c) = self.try_escape_unicode_sequence()? {
                    Ok(c)
                } else if !self.flags.unicode {
                    // CharacterEscape :: IdentityEscape :: SourceCharacterIdentityEscape
                    Ok(c)
                } else {
                    self.error("Invalid unicode escape")
                }
            }
            // CharacterEscape :: [~UnicodeMode] LegacyOctalEscapeSequence
            '0'..='7' if !self.flags.unicode => {
                let Some(c1) = self.peek()? else {
                    return Ok(c - '0' as u32);
                };
                let ch1 = to_char_sat(c1);

                match ch {
                    // 0 [lookahead ∈ { 8, 9 }]
                    '0' if ('8'..='9').contains(&ch1) => Ok(0x0),
                    // NonZeroOctalDigit [lookahead ∉ OctalDigit]
                    _ if !('0'..='7').contains(&ch1) => Ok(c - '0' as u32),
                    // FourToSeven OctalDigit
                    '4'..='7' => {
                        self.consume(c1)?;
                        Ok((c - '0' as u32) * 8 + c1 - '0' as u32)
                    }
                    // ZeroToThree OctalDigit [lookahead ∉ OctalDigit]
                    // ZeroToThree OctalDigit OctalDigit
                    '0'..='3' => {
                        self.consume(c1)?;
                        if self
                            .peek()?
                            .map(|c2| ('0'..='7').contains(&to_char_sat(c2)))
                            == Some(true)
                        {
                            let c2 = self.next()?.expect("char was not next");
                            Ok((c - '0' as u32) * 64 + (c1 - '0' as u32) * 8 + c2 - '0' as u32)
                        } else {
                            Ok((c - '0' as u32) * 8 + c1 - '0' as u32)
                        }
                    }
                    _ => unreachable!(),
                }
            }
            // CharacterEscape :: IdentityEscape :: [+UnicodeMode] SyntaxCharacter
            // CharacterEscape :: IdentityEscape :: [+UnicodeMode] /
            '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
            | '/' => Ok(c),
            // CharacterEscape :: IdentityEscape :: SourceCharacterIdentityEscape
            _ if !self.flags.unicode => Ok(c),
            _ => self.error("Invalid character escape"),
        }
    }

    // AtomEscape
    fn consume_atom_escape(&mut self) -> Result<ParsedNode, CompileError> {
        let Some(c) = self.peek()? else {
            return self.error("Incomplete escape");
        };
        match to_char_sat(c) {
            'd' | 'D' => {
                self.consume(c)?;
                self.leaf(make_bracket_class(
                    CharacterClassType::Digits,
                    c == 'd' as u32,
                    self.flags.icase,
                    self.budget,
                )?)
            }

            's' | 'S' => {
                self.consume(c)?;
                self.leaf(make_bracket_class(
                    CharacterClassType::Spaces,
                    c == 's' as u32,
                    self.flags.icase,
                    self.budget,
                )?)
            }

            'w' | 'W' => {
                self.consume(c)?;
                self.leaf(make_bracket_class(
                    CharacterClassType::Words,
                    c == 'w' as u32,
                    self.flags.icase,
                    self.budget,
                )?)
            }

            // ClassEscape :: CharacterClassEscape :: [+UnicodeMode] p{ UnicodePropertyValueExpression }
            // ClassEscape :: CharacterClassEscape :: [+UnicodeMode] P{ UnicodePropertyValueExpression }
            'p' | 'P' if self.flags.unicode || self.flags.unicode_sets => {
                self.consume(c)?;
                let negate = c == 'P' as u32;
                let property_escape = self.try_consume_unicode_property_escape()?;
                match property_escape {
                    PropertyEscapeKind::CharacterClass(s) => {
                        let mut cps = self.property_set(s)?;
                        if self.flags.icase {
                            // Per ES2024: apply SimpleCaseFolding to the property set first.
                            // For \P (inverted): complement before expansion so that case
                            // variants of the complement are included (existential quantifier).
                            if negate {
                                cps = unicode::try_add_icase_code_points(
                                    cps.try_inverted(self.budget, CompileSite::CodePointSet)?,
                                    self.budget,
                                )?;
                            } else {
                                cps = unicode::try_add_icase_code_points(cps, self.budget)?;
                            }
                            self.leaf(ir::Node::Bracket(BracketContents { invert: false, cps }))
                        } else {
                            self.leaf(ir::Node::Bracket(BracketContents {
                                invert: negate,
                                cps,
                            }))
                        }
                    }
                    PropertyEscapeKind::StringSet(_) if negate => {
                        self.error("Invalid character escape")
                    }
                    PropertyEscapeKind::StringSet(strings) => self.leaf(ir::Node::StringSet {
                        alternatives: ClassSetAlternativeStrings::from_slices(
                            strings,
                            self.budget,
                        )?
                        .0
                        .into_vec(),
                        icase: self.flags.icase,
                    }),
                }
            }

            // [+UnicodeMode] DecimalEscape
            // Note: This is a backreference.
            '1'..='9' if self.flags.unicode => {
                let group = self.try_consume_decimal_integer_literal()?.unwrap();
                if group <= self.group_count_max as usize {
                    self.leaf(ir::Node::BackRef {
                        group: group as u32,
                        icase: self.flags.icase,
                    })
                } else {
                    self.error("Invalid character escape")
                }
            }

            // [~UnicodeMode] DecimalEscape but only if the CapturingGroupNumber of DecimalEscape
            //    is ≤ CountLeftCapturingParensWithin(the Pattern containing DecimalEscape)
            // Note: This could be either a backreference, a legacy octal escape or an identity escape.
            '1'..='9' => {
                let input = self.input.clone();
                let group = self.try_consume_decimal_integer_literal()?.unwrap();

                if group <= self.group_count_max as usize {
                    self.leaf(ir::Node::BackRef {
                        group: group as u32,
                        icase: self.flags.icase,
                    })
                } else {
                    self.input = input;
                    let c = self.consume_character_escape()?;
                    Ok(self.char_node(c)?)
                }
            }

            // [+NamedCaptureGroups] k GroupName
            'k' if self.flags.unicode || !self.named_group_indices.is_empty() => {
                // The sequence `\k` must be the start of a backreference to a named capture group.
                // Note multiple capture groups may have the same name; we must map all of them to their indices.
                self.consume('k')?;
                // Must have a valid group name.
                let Some(group_name) = self.try_consume_named_capture_group_name()? else {
                    return self.error("Invalid named backreference syntax");
                };
                // The group name must be the name of a previously defined capture group.
                let Some(group_indices) = self.named_group_indices.get(&group_name, self.budget)?
                else {
                    return self.error_fmt(format_args!(
                        "Backreference to invalid named capture group: {}",
                        group_name
                    ));
                };
                // Only one same-name group can participate. Concatenation consumes
                // that capture while unmatched references contribute the empty string.
                if group_indices.len() == 1 {
                    return self.leaf(ir::Node::BackRef {
                        group: group_indices[0] + 1,
                        icase: self.flags.icase,
                    });
                }
                let mut references = CompileVec::new();
                references.reserve(group_indices.len(), self.budget)?;
                for &group_index in group_indices.iter() {
                    references.push(
                        self.leaf(ir::Node::BackRef {
                            group: group_index + 1,
                            icase: self.flags.icase,
                        })?,
                        self.budget,
                    )?;
                }
                make_cat(references, self.budget)
            }

            // [~NamedCaptureGroups] k GroupName
            'k' => {
                self.consume('k')?;
                Ok(self.char_node(c)?)
            }

            _ => {
                let c = self.consume_character_escape()?;
                Ok(self.char_node(c)?)
            }
        }
    }

    #[allow(clippy::branches_sharing_code)]
    #[allow(
        clippy::excessive_nesting,
        reason = "Preserve reviewed Unicode escape parsing control flow."
    )]
    fn try_escape_unicode_sequence(&mut self) -> Result<Option<u32>, CompileError> {
        let mut orig_input = self.input.clone();

        // Support \u{X..X} (Unicode CodePoint)
        Ok(if self.try_consume('{')? {
            let mut s = CompileString::new();
            loop {
                match self.next()?.and_then(char::from_u32) {
                    Some('}') => break,
                    Some(c) => s.push(c, self.budget)?,
                    None => {
                        // Surrogates not supported in code point escapes.
                        self.input = orig_input;
                        return Ok(None);
                    }
                }
            }

            self.scan_work(s.len())?;
            match u32::from_str_radix(&s, 16) {
                Ok(u) => {
                    if u > 0x10_FFFF {
                        self.input = orig_input;
                        None
                    } else {
                        Some(u)
                    }
                }
                _ => {
                    self.input = orig_input;
                    None
                }
            }
        } else {
            // Hex4Digits
            let mut s = CompileString::new();
            for _ in 0..4 {
                if let Some(c) = self.next()?.and_then(char::from_u32) {
                    s.push(c, self.budget)?;
                } else {
                    // Surrogates are not hex digits.
                    self.input = orig_input;
                    return Ok(None);
                }
            }
            self.scan_work(s.len())?;
            match u16::from_str_radix(&s, 16) {
                Ok(u) => {
                    if (0xD800..=0xDBFF).contains(&u) {
                        // Found a high surrogate. Try to parse a low surrogate next
                        // to see if we can rebuild the original `char`

                        if !self.try_consume_str("\\u")? {
                            return Ok(Some(u as u32));
                        }
                        orig_input = self.input.clone();

                        // A poor man's try block to handle the backtracking
                        // in a single place instead of every time we want to return.
                        // This allows us to use `?` within the inner block without returning
                        // from the entire parent function.
                        let result = (|| -> Result<Option<u32>, CompileError> {
                            let mut s = CompileString::new();
                            for _ in 0..4 {
                                let Some(c) = self.next()?.and_then(char::from_u32) else {
                                    return Ok(None);
                                };
                                s.push(c, self.budget)?;
                            }

                            self.scan_work(s.len())?;
                            let Ok(uu) = u16::from_str_radix(&s, 16) else {
                                return Ok(None);
                            };
                            let Some(Ok(ch)) = char::decode_utf16([u, uu]).next() else {
                                return Ok(None);
                            };
                            Ok(Some(u32::from(ch)))
                        })()?;

                        result.or_else(|| {
                            self.input = orig_input;
                            Some(u as u32)
                        })
                    } else {
                        // If `u` is not a surrogate or is a low surrogate we can directly return it,
                        // since all paired low surrogates should have been handled above.
                        Some(u as u32)
                    }
                }
                _ => {
                    self.input = orig_input;
                    None
                }
            }
        })
    }

    fn try_consume_named_capture_group_name(&mut self) -> Result<Option<String>, CompileError> {
        if !self.try_consume('<')? {
            return Ok(None);
        }

        let orig_input = self.input.clone();
        let mut group_name = CompileString::new();

        if let Some(mut c) = self.next()?.and_then(char::from_u32) {
            if c == '\\' && self.try_consume('u')? {
                if let Some(escaped) = self.try_escape_unicode_sequence()?.and_then(char::from_u32)
                {
                    c = escaped;
                } else {
                    self.input = orig_input;
                    return Ok(None);
                }
            }

            if try_interval_contains(id_start_ranges(), c.into(), self.budget)?
                || c == '$'
                || c == '_'
            {
                group_name.push(c, self.budget)?;
            } else {
                self.input = orig_input;
                return Ok(None);
            }
        } else {
            self.input = orig_input;
            return Ok(None);
        }

        loop {
            if let Some(mut c) = self.next()?.and_then(char::from_u32) {
                if c == '\\' && self.try_consume('u')? {
                    if let Some(escaped) =
                        self.try_escape_unicode_sequence()?.and_then(char::from_u32)
                    {
                        c = escaped;
                    } else {
                        self.input = orig_input;
                        return Ok(None);
                    }
                }

                if c == '>' {
                    break;
                }

                if try_interval_contains(id_continue_ranges(), c.into(), self.budget)? || c == '$' || c == '_' || c == '\u{200C}' /* <ZWNJ> */ || c == '\u{200D}'
                /* <ZWJ> */
                {
                    group_name.push(c, self.budget)?;
                } else {
                    self.input = orig_input;
                    return Ok(None);
                }
            } else {
                self.input = orig_input;
                return Ok(None);
            }
        }

        Ok(Some(group_name.into_string()))
    }

    // Quickly parse all capture groups.
    // Per TC39 proposal, duplicate named groups are allowed in different alternatives.
    fn parse_capture_groups(&mut self) -> Result<(), CompileError> {
        let orig_input = self.input.clone();

        // Pass 1: Collect all named capture groups with their alternative paths
        let named_group_locations = self.collect_named_group_locations()?;

        // Pass 2: Check for conflicts (duplicates in the same alternative path)
        self.check_duplicate_conflicts(&named_group_locations)?;

        self.input = orig_input;

        Ok(())
    }

    /// Pass 1: Collect all named capture groups and record which alternative path each appears in.
    fn collect_named_group_locations(
        &mut self,
    ) -> Result<NameTable<CompileVec<AlternativePath>>, CompileError> {
        // Track parenthesis depth and alternative index at each depth
        let mut paren_depth: usize = 0;
        let mut alt_indices = CompileVec::new();
        alt_indices.push(0_usize, self.budget)?;

        // Map from group name to all alternative paths where it appears
        let mut named_group_locations: NameTable<CompileVec<AlternativePath>> =
            NameTable::new(self.budget);

        loop {
            match self.next()?.map(to_char_sat) {
                Some('\\') => {
                    self.next()?;
                    continue;
                }
                Some('[') if self.flags.unicode_sets => {
                    // In unicode_sets (`v`) mode character classes can nest,
                    // e.g. `[[a][b]]`, so track bracket depth and stop only at
                    // the matching `]`.
                    let mut depth = 1usize;
                    loop {
                        match self.next()?.map(to_char_sat) {
                            Some('\\') => {
                                self.next()?;
                                continue;
                            }
                            Some('[') => depth += 1,
                            Some(']') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Some(_) => continue,
                            None => break,
                        }
                    }
                }
                Some('[') => loop {
                    match self.next()?.map(to_char_sat) {
                        Some('\\') => {
                            self.next()?;
                            continue;
                        }
                        Some(']') => break,
                        Some(_) => continue,
                        None => break,
                    }
                },
                Some('(') => {
                    // Determine whether we're a capturing group, and optionally the name.
                    let is_capturing;
                    let group_name;
                    if self.try_consume_str("?")? {
                        group_name = self.try_consume_named_capture_group_name()?;
                        is_capturing = group_name.is_some(); // (?:, (?=, (?!, etc. are non-capturing.
                    } else {
                        is_capturing = true;
                        group_name = None; // Unnamed capture group
                    }

                    if let Some(name) = group_name {
                        // Build current alternative path from depth 0 to current depth.
                        let mut segments = CompileVec::new();
                        for (d, index) in alt_indices.iter().copied().enumerate() {
                            segments.push((d, index), self.budget)?;
                        }

                        // Record this location.
                        named_group_locations
                            .get_or_insert(copy_string(&name, self.budget)?, self.budget)?
                            .push(
                                AlternativePath {
                                    segments: segments.into_vec(),
                                },
                                self.budget,
                            )?;

                        // Store all occurrences in named_group_indices.
                        self.named_group_indices
                            .get_or_insert(name, self.budget)?
                            .push(self.group_count_max, self.budget)?;
                    }

                    if is_capturing {
                        self.group_count_max =
                            if self.group_count_max + 1 > MAX_CAPTURE_GROUPS as u32 {
                                MAX_CAPTURE_GROUPS as u32
                            } else {
                                self.group_count_max + 1
                            };
                    }

                    // Entering a new group.
                    paren_depth += 1;
                    alt_indices.push(0, self.budget)?;
                }
                Some(')') => {
                    // Exiting a group
                    if paren_depth > 0 {
                        alt_indices.pop(self.budget)?;
                        paren_depth -= 1;
                    }
                }
                Some('|') => {
                    // Moving to next alternative at current depth
                    alt_indices[paren_depth] += 1;
                }
                Some(_) => continue,
                None => break,
            }
        }

        Ok(named_group_locations)
    }

    /// Pass 2: Check that named groups with the same name don't conflict.
    /// Groups conflict if they appear in the same alternative path.
    /// Per TC39 proposal, `/(?<a>x)|(?<a>y)/` is valid (different alternatives),
    /// but `/(?<a>x)(?<a>y)/` is invalid (same alternative).
    fn check_duplicate_conflicts(
        &self,
        named_group_locations: &NameTable<CompileVec<AlternativePath>>,
    ) -> Result<(), CompileError> {
        named_group_locations.for_each_value(|paths| {
            self.read_work()?;
            // Check each pair of paths for this group name
            for i in 0..paths.len() {
                for j in (i + 1)..paths.len() {
                    if paths[i].conflicts_with(&paths[j], self.budget)? {
                        return self.error("Duplicate capture group name");
                    }
                }
            }
            Ok(())
        })
    }

    fn try_consume_unicode_property_escape(&mut self) -> Result<PropertyEscapeKind, CompileError> {
        if !self.try_consume('{')? {
            return self.error("Invalid character at property escape start");
        }

        let mut buffer = CompileString::new();
        let mut name = None;

        while let Some(c) = self.peek()?.and_then(char::from_u32) {
            match c {
                '}' => {
                    self.consume(c)?;
                    let Some(value) = try_unicode_property_from_str(
                        &buffer,
                        name,
                        self.flags.unicode_sets,
                        self.budget,
                    )?
                    else {
                        break;
                    };
                    return Ok(value);
                }
                '=' if name.is_none() => {
                    self.consume(c)?;
                    let Some(n) = try_unicode_property_name_from_str(&buffer, self.budget)? else {
                        break;
                    };
                    name = Some(n);
                    buffer.clear();
                }
                c if c.is_ascii_alphanumeric() || c == '_' => {
                    self.consume(c)?;
                    buffer.push(c, self.budget)?;
                }
                _ => break,
            }
        }

        self.error("Invalid property name")
    }

    fn finalize(&self, mut re: ir::Regex) -> Result<ir::Regex, CompileError> {
        debug_assert!(self.loop_count <= MAX_LOOPS as u32);
        debug_assert!(self.group_count as usize <= MAX_CAPTURE_GROUPS);
        if self.has_lookbehind {
            ir::try_walk_mut(
                false,
                re.flags.unicode,
                &mut re.node,
                self.budget,
                CompileSite::ParserFinalize,
                &mut |node, walk| {
                    if walk.in_lookbehind {
                        if let (ir::Node::Cat(nodes), Some(budget)) = (&node, self.budget) {
                            budget
                                .at(CompileSite::ParserFinalize)
                                .reserve(nodes.len(), 0)?;
                        }
                    }
                    node.reverse_cats(walk);
                    Ok(())
                },
            )?;
        }
        Ok(re)
    }
}

/// Try parsing a given pattern.
/// Return the resulting IR regex, or an error.
pub fn try_parse<I>(pattern: I, flags: api::Flags) -> Result<ir::Regex, Error>
where
    I: Iterator<Item = u32> + Clone,
{
    try_parse_with_budget(pattern, flags, None).map_err(|error| match error {
        CompileError::Syntax(error) => error,
        CompileError::Limit(_) => unreachable!("unlimited parser has no resource allowance"),
    })
}

pub(crate) fn try_parse_with_budget<I>(
    pattern: I,
    flags: api::Flags,
    budget: Option<&CompileBudget>,
) -> Result<ir::Regex, CompileError>
where
    I: Iterator<Item = u32> + Clone,
{
    let mut p = Parser {
        budget,
        input: pattern.peekable(),
        flags,
        loop_count: 0,
        group_count: 0,
        named_group_indices: NameTable::new(budget),
        group_count_max: 0,
        has_lookbehind: false,
        depth: 0,
    };
    p.try_parse()
}

#[cfg(test)]
mod compile_budget_tests {
    use super::*;
    use core::cell::Cell;

    fn class_contents<'a>(
        text: &'a str,
        budget: &'a CompileBudget,
    ) -> Parser<'a, impl Iterator<Item = u32> + Clone + 'a> {
        Parser {
            budget: Some(budget),
            input: text.chars().map(u32::from).peekable(),
            flags: api::Flags::from("v"),
            loop_count: 0,
            group_count: 0,
            named_group_indices: NameTable::new(Some(budget)),
            group_count_max: 0,
            has_lookbehind: false,
            depth: 0,
        }
    }

    #[test]
    fn single_ampersand_refuses_unfunded_set_insertion() {
        let denied = CompileBudget::new(1_000_000, 0, 128);
        let result = class_contents("a&]", &denied).consume_class_set_expression();
        assert!(
            matches!(result, Err(CompileError::Limit(CompileLimit::Allocation))),
            "single ampersand insertion succeeded with no set-storage allowance"
        );
        assert_eq!(denied.exhausted_at(), Some(CompileSite::CodePointSet));
        assert!(matches!(
            class_contents("a&]", &denied).consume_class_set_expression(),
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        let funded = CompileBudget::new(1_000_000, 1_000_000, 128);
        let result = class_contents("a&]", &funded)
            .consume_class_set_expression()
            .unwrap();
        assert!(result.codepoints.contains(u32::from('&')));
    }

    #[cfg(all(feature = "std", feature = "utf16"))]
    #[test]
    fn single_ampersand_public_v_route_refuses_at_set_storage() {
        // Source collection requests 24 bytes; the capture prepass owns one
        // two-slot usize buffer. No set-storage allowance remains after those.
        let prefix_storage = 24 + 2 * core::mem::size_of::<usize>();
        let mut denied = CompileBudget::new(1_000_000, prefix_storage, 128);
        let result =
            api::Regex::from_unicode_with_limits("[a&]".chars().map(u32::from), "v", &mut denied);
        assert!(matches!(
            result,
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        assert_eq!(denied.exhausted_at(), Some(CompileSite::CodePointSet));
        assert!(matches!(
            api::Regex::from_unicode_with_limits("a".chars().map(u32::from), "", &mut denied),
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        assert_eq!(denied.exhausted_at(), Some(CompileSite::CodePointSet));
        let mut funded = CompileBudget::new(1_000_000, 1_000_000, 128);
        let regex =
            api::Regex::from_unicode_with_limits("[a&]".chars().map(u32::from), "v", &mut funded)
                .unwrap();
        assert_eq!(regex.find("&").unwrap().range(), 0..1);
    }

    #[test]
    fn decimal_presence_preserves_consumption_and_saturation() {
        for (text, expected, next) in [
            ("", None, None),
            ("x", None, Some('x')),
            ("0!", Some(0), Some('!')),
            ("00012!", Some(12), Some('!')),
            ("42", Some(42), None),
            (
                "9999999999999999999999999999999999999999!",
                Some(usize::MAX),
                Some('!'),
            ),
            ("０x", None, Some('０')),
        ] {
            let budget = CompileBudget::new(1_000_000, 1_000_000, 128);
            let mut parser = Parser {
                budget: Some(&budget),
                input: text.chars().map(u32::from).peekable(),
                flags: api::Flags::default(),
                loop_count: 0,
                group_count: 0,
                named_group_indices: NameTable::new(Some(&budget)),
                group_count_max: 0,
                has_lookbehind: false,
                depth: 0,
            };
            assert_eq!(
                parser.try_consume_decimal_integer_literal().unwrap(),
                expected,
                "{text}"
            );
            assert_eq!(parser.peek().unwrap(), next.map(u32::from), "{text}");
        }
    }

    #[test]
    fn parser_refuses_before_reading_without_work() {
        let reads = Cell::new(0);
        let input = [u32::from('a')]
            .into_iter()
            .inspect(|_| reads.set(reads.get() + 1));
        let budget = CompileBudget::new(0, 1_000_000, 100);
        let result = try_parse_with_budget(input, api::Flags::default(), Some(&budget));
        assert!(matches!(
            result,
            Err(CompileError::Limit(CompileLimit::Work))
        ));
        assert_eq!(reads.get(), 0);

        let budget = CompileBudget::new(1_000_000, 1_000_000, 100);
        let input = [u32::from('a')]
            .into_iter()
            .inspect(|_| reads.set(reads.get() + 1));
        assert!(try_parse_with_budget(input, api::Flags::default(), Some(&budget)).is_ok());
        assert!(reads.get() > 0);
    }
    #[cfg(all(feature = "std", feature = "utf16"))]
    #[test]
    fn property_storage_refusal_identifies_materialization_stage() {
        let mut budget = CompileBudget::new(1_000_000, 4_096, 100);
        let result = api::Regex::from_unicode_with_limits(
            r"\p{Letter}".chars().map(u32::from),
            "u",
            &mut budget,
        );
        assert!(matches!(
            result,
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        assert_eq!(budget.exhausted_at(), Some(CompileSite::PropertyCopy));
        assert!(matches!(
            api::Regex::from_unicode_with_limits("a".chars().map(u32::from), "", &mut budget),
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        assert_eq!(budget.exhausted_at(), Some(CompileSite::PropertyCopy));
    }

    #[cfg(all(feature = "std", feature = "utf16"))]
    #[test]
    fn repeated_body_storage_refusal_must_reach_optimizer() {
        let mut budget = CompileBudget::new(1_000_000, 4_096, 100);
        let result = api::Regex::from_unicode_with_limits(
            "(?:(?:abcdef){4}){4}".chars().map(u32::from),
            "u",
            &mut budget,
        );
        assert!(matches!(
            result,
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        assert_eq!(budget.exhausted_at(), Some(CompileSite::Optimizer));
    }

    #[cfg(all(feature = "std", feature = "utf16"))]
    #[test]
    fn optimizer_refuses_extra_unroll_height_before_replacement() {
        let mut small = CompileBudget::new(1_000_000, 1_000_000, 4);
        let result = api::Regex::from_unicode_with_limits(
            "(?:a|b){2,}".chars().map(u32::from),
            "",
            &mut small,
        );
        assert!(matches!(
            result,
            Err(CompileError::Limit(CompileLimit::Depth))
        ));
        assert_eq!(small.exhausted_at(), Some(CompileSite::Optimizer));
        let mut generous = CompileBudget::new(1_000_000, 1_000_000, 8);
        let regex = api::Regex::from_unicode_with_limits(
            "(?:a|b){2,}".chars().map(u32::from),
            "",
            &mut generous,
        )
        .unwrap();
        assert_eq!(regex.find("ab").unwrap().range(), 0..2);
    }

    #[test]
    fn class_string_sort_retains_ties_and_refuses_scratch() {
        let strings: &[&[u32]] = &[&[97, 99], &[108, 111, 110, 103], &[97, 98], &[122]];
        let input = ClassSetAlternativeStrings::from_slices(strings, None).unwrap();
        let denied = CompileBudget::new(1_000_000, 0, 128);
        assert!(matches!(
            input.into_node(false, Some(&denied)),
            Err(CompileError::Limit(CompileLimit::Allocation))
        ));
        assert_eq!(denied.exhausted_at(), Some(CompileSite::ClassSet));
        let input = ClassSetAlternativeStrings::from_slices(strings, None).unwrap();
        let generous = CompileBudget::new(1_000_000, 1_000_000, 128);
        let ir::Node::StringSet { alternatives, .. } =
            input.into_node(false, Some(&generous)).unwrap()
        else {
            panic!("expected string alternatives")
        };
        assert_eq!(
            alternatives.iter().map(|s| s.as_ref()).collect::<Vec<_>>(),
            vec![
                &[108, 111, 110, 103][..],
                &[97, 99][..],
                &[97, 98][..],
                &[122][..]
            ]
        );
    }
}
