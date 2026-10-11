//! Explicitly admitted compiler storage.

use crate::budget::{CompileBudget, CompileError, CompileLimit, CompileSite};
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, string::String, vec::Vec};
use core::{
    mem,
    ops::{Deref, DerefMut, Range},
};

pub(crate) struct CompileVec<T> {
    values: Vec<T>,
    admitted: usize,
    site: CompileSite,
}

impl<T: core::fmt::Debug> core::fmt::Debug for CompileVec<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.values.fmt(f)
    }
}
impl<T: Clone> Clone for CompileVec<T> {
    fn clone(&self) -> Self {
        Self {
            values: self.values.clone(),
            admitted: self.values.len(),
            site: self.site,
        }
    }
}
impl<T: PartialEq> PartialEq for CompileVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}
impl<T: Eq> Eq for CompileVec<T> {}
impl<'a, T> IntoIterator for &'a CompileVec<T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}
impl<'a, T> IntoIterator for &'a mut CompileVec<T> {
    type Item = &'a mut T;
    type IntoIter = core::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.iter_mut()
    }
}

impl<T> CompileVec<T> {
    pub(crate) fn new() -> Self {
        Self {
            values: Vec::new(),
            admitted: 0,
            site: CompileSite::ParserStorage,
        }
    }

    pub(crate) fn new_at(site: CompileSite) -> Self {
        Self {
            values: Vec::new(),
            admitted: 0,
            site,
        }
    }

    pub(crate) fn from_vec(values: Vec<T>) -> Self {
        // Capacity not recorded by the caller is not a new resource allowance.
        let admitted = values.len();
        Self {
            values,
            admitted,
            site: CompileSite::ParserStorage,
        }
    }

    pub(crate) fn reserve(
        &mut self,
        additional: usize,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        let Some(budget) = budget else {
            self.values.reserve(additional);
            self.admitted = self.values.capacity();
            return Ok(());
        };
        budget.at(self.site).check()?;
        let required = self
            .values
            .len()
            .checked_add(additional)
            .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
        if required <= self.admitted {
            return Ok(());
        }
        let capacity = self
            .admitted
            .max(1)
            .checked_mul(2)
            .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?
            .max(required);
        let bytes = capacity
            .checked_mul(mem::size_of::<T>())
            .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
        budget.at(self.site).reserve(self.values.len(), bytes)?;
        self.values
            .try_reserve_exact(capacity - self.values.len())
            .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
        self.admitted = capacity;
        Ok(())
    }

    pub(crate) fn push(
        &mut self,
        value: T,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        if let Some(budget) = budget {
            budget.at(self.site).reserve(1, 0)?;
        }
        self.reserve(1, budget)?;
        self.values.push(value);
        Ok(())
    }

    pub(crate) fn insert(
        &mut self,
        index: usize,
        value: T,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        if let Some(budget) = budget {
            budget
                .at(self.site)
                .reserve(self.values.len() - index + 1, 0)?;
        }
        self.reserve(1, budget)?;
        self.values.insert(index, value);
        Ok(())
    }

    pub(crate) fn split_off(
        &mut self,
        at: usize,
        budget: Option<&CompileBudget>,
    ) -> Result<Self, CompileError> {
        let mut suffix = Self::new_at(self.site);
        let length = self.values.len() - at;
        suffix.reserve(length, budget)?;
        if let Some(budget) = budget {
            budget.at(self.site).reserve(length, 0)?;
        }
        suffix.values.extend(self.values.drain(at..));
        Ok(suffix)
    }

    pub(crate) fn remove_range(
        &mut self,
        range: Range<usize>,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        if let Some(budget) = budget {
            budget
                .at(self.site)
                .reserve(self.values.len() - range.start, 0)?;
        }
        self.values.drain(range);
        Ok(())
    }

    pub(crate) fn pop(
        &mut self,
        budget: Option<&CompileBudget>,
    ) -> Result<Option<T>, CompileError> {
        if let Some(budget) = budget {
            budget.at(self.site).reserve(1, 0)?;
        }
        Ok(self.values.pop())
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }
    pub(crate) fn as_slice(&self) -> &[T] {
        self.values.as_slice()
    }
    pub(crate) fn set_site(&mut self, site: CompileSite) {
        self.site = site;
    }

    pub(crate) fn into_vec(self) -> Vec<T> {
        self.values
    }
}

impl<T> Default for CompileVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Deref for CompileVec<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<T> DerefMut for CompileVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

impl<T> IntoIterator for CompileVec<T> {
    type Item = T;
    type IntoIter = <Vec<T> as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

pub(crate) struct CompileString {
    value: String,
    admitted: usize,
}

impl CompileString {
    pub(crate) fn new() -> Self {
        Self {
            value: String::new(),
            admitted: 0,
        }
    }

    pub(crate) fn push(
        &mut self,
        c: char,
        budget: Option<&CompileBudget>,
    ) -> Result<(), CompileError> {
        self.push_at(c, budget, CompileSite::ParserStorage)
    }

    fn push_at(
        &mut self,
        c: char,
        budget: Option<&CompileBudget>,
        site: CompileSite,
    ) -> Result<(), CompileError> {
        let Some(budget) = budget else {
            self.value.push(c);
            self.admitted = self.value.capacity();
            return Ok(());
        };
        budget.at(site).reserve(c.len_utf8(), 0)?;
        let required = self
            .value
            .len()
            .checked_add(c.len_utf8())
            .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
        if required > self.admitted {
            let capacity = self
                .admitted
                .max(1)
                .checked_mul(2)
                .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?
                .max(required);
            budget.at(site).reserve(self.value.len(), capacity)?;
            self.value
                .try_reserve_exact(capacity - self.value.len())
                .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
            self.admitted = capacity;
        }
        self.value.push(c);
        Ok(())
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
    }
    pub(crate) fn into_string(self) -> String {
        self.value
    }
}

impl Deref for CompileString {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

pub(crate) fn syntax_error<T>(
    args: core::fmt::Arguments<'_>,
    budget: Option<&CompileBudget>,
) -> Result<T, CompileError> {
    if let Some(budget) = budget {
        budget.at(CompileSite::Diagnostic).check()?;
    }
    struct Writer<'a> {
        text: CompileString,
        budget: Option<&'a CompileBudget>,
    }
    impl core::fmt::Write for Writer<'_> {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            for c in text.chars() {
                self.text
                    .push_at(c, self.budget, CompileSite::Diagnostic)
                    .map_err(|_| core::fmt::Error)?;
            }
            Ok(())
        }
    }
    let mut writer = Writer {
        text: CompileString::new(),
        budget,
    };
    use core::fmt::Write;
    if writer.write_fmt(args).is_err() {
        if let Some(budget) = budget {
            budget.check()?;
        }
        unreachable!("built-in syntax formatting failed without budget exhaustion");
    }
    Err(CompileError::Syntax(crate::Error {
        text: writer.text.into_string(),
    }))
}

pub(crate) fn copy_string(
    text: &str,
    budget: Option<&CompileBudget>,
) -> Result<String, CompileError> {
    let Some(budget) = budget else {
        return Ok(String::from(text));
    };
    budget
        .at(CompileSite::ParserStorage)
        .reserve(text.len(), text.len())?;
    let mut owned = String::new();
    owned
        .try_reserve_exact(text.len())
        .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
    owned.push_str(text);
    Ok(owned)
}

pub(crate) fn copy_slice<T: Copy>(
    values: &[T],
    budget: Option<&CompileBudget>,
    site: CompileSite,
) -> Result<Vec<T>, CompileError> {
    let Some(budget) = budget else {
        return Ok(values.to_vec());
    };
    budget.at(site).check()?;
    let bytes = values
        .len()
        .checked_mul(mem::size_of::<T>())
        .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
    budget.reserve(values.len(), bytes)?;
    let mut copy = Vec::new();
    copy.try_reserve_exact(values.len())
        .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
    copy.extend_from_slice(values);
    Ok(copy)
}

pub(crate) fn boxed<T>(
    value: T,
    budget: Option<&CompileBudget>,
    site: CompileSite,
) -> Result<Box<T>, CompileError> {
    if let Some(budget) = budget {
        budget.at(site).reserve(1, mem::size_of::<T>())?;
    }
    Ok(Box::new(value))
}

pub(crate) fn boxed_slice<T>(
    values: Vec<T>,
    budget: Option<&CompileBudget>,
    site: CompileSite,
) -> Result<Box<[T]>, CompileError> {
    if let Some(budget) = budget {
        budget.at(site).check()?;
        let bytes = values
            .len()
            .checked_mul(mem::size_of::<T>())
            .ok_or_else(|| budget.refuse(CompileLimit::Allocation))?;
        // Vec-to-box conversion may shrink the backing allocation.
        budget.reserve(values.len(), bytes)?;
    }
    Ok(values.into_boxed_slice())
}

pub(crate) fn copy_boxed_slice<T: Copy>(
    values: &[T],
    budget: Option<&CompileBudget>,
    site: CompileSite,
) -> Result<Box<[T]>, CompileError> {
    boxed_slice(copy_slice(values, budget, site)?, budget, site)
}

pub(crate) fn copy_boxed_str(
    text: &str,
    budget: Option<&CompileBudget>,
    site: CompileSite,
) -> Result<Box<str>, CompileError> {
    let Some(budget) = budget else {
        return Ok(text.into());
    };
    budget.at(site).reserve(text.len(), text.len())?;
    let mut value = String::new();
    value
        .try_reserve_exact(text.len())
        .map_err(|_| budget.refuse(CompileLimit::Allocation))?;
    value.push_str(text);
    budget.reserve(text.len(), text.len())?;
    Ok(value.into_boxed_str())
}

#[cfg(test)]
mod storage_budget_tests {
    use super::*;

    #[test]
    fn admitted_capacity_and_denied_growth_preserve_values() {
        let mut values = CompileVec::<u64>::new();
        let budget = CompileBudget::new(100, 2 * mem::size_of::<u64>(), 100);
        values.push(7, Some(&budget)).unwrap();
        values.push(8, Some(&budget)).unwrap();
        assert_eq!(budget.remaining_allocation(), 0);
        assert_eq!(budget.exhausted(), None);
        assert_eq!(
            values.push(9, Some(&budget)),
            Err(CompileError::Limit(CompileLimit::Allocation))
        );
        assert_eq!(&*values, &[7, 8]);
    }

    #[test]
    fn allocator_capacity_does_not_create_an_allowance() {
        let mut values = CompileVec::<u64>::from_vec(Vec::with_capacity(128));
        let budget = CompileBudget::new(100, 0, 100);
        assert_eq!(
            values.push(7, Some(&budget)),
            Err(CompileError::Limit(CompileLimit::Allocation))
        );
        assert!(values.is_empty());
    }

    #[test]
    fn capacity_arithmetic_refuses_before_allocator_call() {
        let mut values = CompileVec::<u64>::new();
        let budget = CompileBudget::new(usize::MAX, usize::MAX, 100);
        assert_eq!(
            values.reserve(usize::MAX, Some(&budget)),
            Err(CompileError::Limit(CompileLimit::Allocation))
        );
        assert!(values.is_empty());
        assert_eq!(values.values.capacity(), 0);
        assert_eq!(
            values.push(7, Some(&budget)),
            Err(CompileError::Limit(CompileLimit::Allocation))
        );
    }
}
