//! Explicit, sticky work limits for fallible matching.

use core::cell::Cell;

/// Caller-owned work allowance, shared across matching calls and nested assertions.
#[derive(Debug)]
pub struct MatchBudget {
    remaining: Cell<usize>,
    exhausted: Cell<bool>,
}

/// Matching stopped before its work allowance could be exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchError {
    /// Work was exhausted; this is not an ordinary no-match result.
    BudgetExceeded,
}

impl MatchBudget {
    /// Create an allowance that can be reused across the gates of one request.
    pub fn new(work: usize) -> Self {
        Self {
            remaining: Cell::new(work),
            exhausted: Cell::new(false),
        }
    }

    /// Remaining work units; exhaustion is sticky even if another pattern is tried.
    pub fn remaining(&self) -> usize {
        self.remaining.get()
    }

    /// Whether a previous operation refused work.
    pub fn is_exhausted(&self) -> bool {
        self.exhausted.get()
    }

    #[cfg(feature = "utf16")]
    pub(crate) fn check(&self) -> Result<(), MatchError> {
        if self.is_exhausted() {
            Err(MatchError::BudgetExceeded)
        } else {
            Ok(())
        }
    }

    pub(crate) fn spend(&self, work: usize) -> bool {
        if self.is_exhausted() {
            return false;
        }
        let Some(remaining) = self.remaining.get().checked_sub(work) else {
            self.remaining.set(0);
            self.exhausted.set(true);
            return false;
        };
        self.remaining.set(remaining);
        true
    }
}

#[inline]
pub(crate) fn spend(budget: Option<&MatchBudget>, work: usize) -> bool {
    budget.is_none_or(|budget| budget.spend(work))
}

/// The compiler resource whose caller-owned allowance was exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileLimit {
    Work,
    Allocation,
    Depth,
}

/// Syntax and resource refusal remain separate compiler outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Syntax(crate::Error),
    Limit(CompileLimit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompileSite {
    Unspecified,
    #[cfg(all(feature = "std", feature = "utf16"))]
    Input,
    ParserCursor,
    ParserHeight,
    ParserFinalize,
    ParserStorage,
    PropertyCopy,
    CodePointSet,
    Unicode,
    ClassSet,
    Diagnostic,
    Optimizer,
    StartPredicate,
    Emitter,
}

#[cfg(test)]
const SITE_COUNT: usize = 14;

/// Caller-owned compiler allowance shared across compilation phases.
#[derive(Debug)]
pub struct CompileBudget {
    work: Cell<usize>,
    allocation: Cell<usize>,
    depth: usize,
    exhausted: Cell<Option<CompileLimit>>,
    #[cfg(test)]
    site: Cell<CompileSite>,
    #[cfg(test)]
    admitted_by_site: [Cell<(usize, usize)>; SITE_COUNT],
    #[cfg(test)]
    refused_request: Cell<Option<(CompileSite, usize, usize)>>,
}

impl CompileBudget {
    pub fn new(work: usize, allocation_bytes: usize, max_depth: usize) -> Self {
        Self {
            work: Cell::new(work),
            allocation: Cell::new(allocation_bytes),
            depth: max_depth,
            exhausted: Cell::new(None),
            #[cfg(test)]
            site: Cell::new(CompileSite::Unspecified),
            #[cfg(test)]
            admitted_by_site: core::array::from_fn(|_| Cell::new((0, 0))),
            #[cfg(test)]
            refused_request: Cell::new(None),
        }
    }
    pub(crate) fn at(&self, _site: CompileSite) -> &Self {
        #[cfg(test)]
        if self.exhausted.get().is_none() {
            self.site.set(_site);
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn exhausted_at(&self) -> Option<CompileSite> {
        self.exhausted.get().map(|_| self.site.get())
    }

    pub fn remaining_work(&self) -> usize {
        self.work.get()
    }
    pub fn remaining_allocation(&self) -> usize {
        self.allocation.get()
    }
    pub fn max_depth(&self) -> usize {
        self.depth
    }
    pub fn exhausted(&self) -> Option<CompileLimit> {
        self.exhausted.get()
    }
    pub(crate) fn refuse(&self, limit: CompileLimit) -> CompileError {
        let limit = self.exhausted.get().unwrap_or(limit);
        self.exhausted.set(Some(limit));
        CompileError::Limit(limit)
    }

    pub(crate) fn reserve(&self, work: usize, allocation: usize) -> Result<(), CompileError> {
        self.check()?;
        let remaining_work = self.work.get().checked_sub(work).ok_or_else(|| {
            #[cfg(test)]
            self.refused_request
                .set(Some((self.site.get(), work, allocation)));
            self.work.set(0);
            self.refuse(CompileLimit::Work)
        })?;
        let remaining_allocation =
            self.allocation
                .get()
                .checked_sub(allocation)
                .ok_or_else(|| {
                    #[cfg(test)]
                    self.refused_request
                        .set(Some((self.site.get(), work, allocation)));
                    self.allocation.set(0);
                    self.refuse(CompileLimit::Allocation)
                })?;
        #[cfg(test)]
        {
            let entry = &self.admitted_by_site[self.site.get() as usize];
            let (prior_work, prior_allocation) = entry.get();
            entry.set((prior_work + work, prior_allocation + allocation));
        }
        self.work.set(remaining_work);
        self.allocation.set(remaining_allocation);
        Ok(())
    }

    pub(crate) fn check(&self) -> Result<(), CompileError> {
        self.exhausted()
            .map_or(Ok(()), |limit| Err(CompileError::Limit(limit)))
    }
}

#[cfg(all(test, feature = "std", feature = "utf16"))]
mod admission_measurement {
    use super::*;

    #[test]
    fn ten_literal_admission_components() {
        let sites = [
            CompileSite::Unspecified,
            CompileSite::Input,
            CompileSite::ParserCursor,
            CompileSite::ParserHeight,
            CompileSite::ParserFinalize,
            CompileSite::ParserStorage,
            CompileSite::PropertyCopy,
            CompileSite::CodePointSet,
            CompileSite::Unicode,
            CompileSite::ClassSet,
            CompileSite::Diagnostic,
            CompileSite::Optimizer,
            CompileSite::StartPredicate,
            CompileSite::Emitter,
        ];
        for allowance in [4_096, 1_000_000] {
            let mut budget = CompileBudget::new(1_000_000, allowance, 100);
            let result = crate::Regex::from_unicode_with_limits(
                "abcdefghij".chars().map(u32::from),
                "u",
                &mut budget,
            );
            println!(
                "ADMISSION allowance={allowance} success={} refused={:?}",
                result.is_ok(),
                budget.refused_request.get()
            );
            for (site, entry) in sites.iter().zip(budget.admitted_by_site.iter()) {
                let (work, bytes) = entry.get();
                if work != 0 || bytes != 0 {
                    println!("COMPONENT {site:?} work={work} allocation_bytes={bytes}");
                }
            }
            if allowance == 4_096 {
                assert!(matches!(
                    result,
                    Err(CompileError::Limit(CompileLimit::Allocation))
                ));
                assert_eq!(budget.exhausted_at(), Some(CompileSite::Emitter));
            } else {
                assert_eq!(result.unwrap().find("abcdefghij").unwrap().range(), 0..10);
            }
        }
    }
}
