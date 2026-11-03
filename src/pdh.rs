//! Thin wrapper over Windows Performance Data Helper (PDH) for wildcard
//! counters such as `\Processor Information(*)\% Processor Performance`.
//!
//! Rate counters need two collections before they yield a value, so callers
//! keep one [`CounterQuery`] alive and call [`CounterQuery::collect`] every
//! interval. Readable without administrator rights.

use anyhow::{Result, bail};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{HSTRING, PCWSTR};

pub struct CounterQuery {
    query: PDH_HQUERY,
    counters: Vec<PDH_HCOUNTER>,
}

// PDH handles may be used from the sampler thread only; they are plain opaque
// handles with no thread affinity.
unsafe impl Send for CounterQuery {}

impl CounterQuery {
    /// Opens a query and adds every path (English names, wildcards allowed).
    pub fn new(paths: &[&str]) -> Result<Self> {
        let mut query = PDH_HQUERY::default();
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if status != 0 {
            bail!("PdhOpenQueryW = {:#x}", status);
        }
        let mut q = Self {
            query,
            counters: Vec::with_capacity(paths.len()),
        };
        for path in paths {
            let mut counter = PDH_HCOUNTER::default();
            let wide = HSTRING::from(*path);
            let status = unsafe { PdhAddEnglishCounterW(q.query, &wide, 0, &mut counter) };
            if status != 0 {
                bail!("PdhAddEnglishCounterW({path}) = {:#x}", status);
            }
            q.counters.push(counter);
        }
        Ok(q)
    }

    /// Takes one sample for all counters. Values become readable after the
    /// second call for rate counters.
    pub fn collect(&mut self) -> Result<()> {
        let status = unsafe { PdhCollectQueryData(self.query) };
        if status != 0 {
            bail!("PdhCollectQueryData = {:#x}", status);
        }
        Ok(())
    }

    /// Instance name and value for counter `index` (position in `paths`).
    /// Instances whose value is not yet valid are skipped.
    pub fn values(&self, index: usize) -> Result<Vec<(String, f64)>> {
        let Some(&counter) = self.counters.get(index) else {
            bail!("counter index {index} out of range");
        };
        let mut size = 0u32;
        let mut count = 0u32;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
        };
        if status != PDH_MORE_DATA {
            if status == 0 {
                return Ok(Vec::new());
            }
            bail!("PdhGetFormattedCounterArrayW(size) = {:#x}", status);
        }
        let mut buf = vec![0u8; size as usize];
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
            )
        };
        if status != 0 {
            bail!("PdhGetFormattedCounterArrayW = {:#x}", status);
        }
        let items = unsafe {
            std::slice::from_raw_parts(
                buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                count as usize,
            )
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let st = item.FmtValue.CStatus;
            if st != PDH_CSTATUS_VALID_DATA && st != PDH_CSTATUS_NEW_DATA {
                continue;
            }
            let name = unsafe { item.szName.to_string() }.unwrap_or_default();
            let value = unsafe { item.FmtValue.Anonymous.doubleValue };
            out.push((name, value));
        }
        Ok(out)
    }
}

impl Drop for CounterQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_processor_frequency_readable() {
        let mut q = CounterQuery::new(&[r"\Processor Information(*)\Processor Frequency"]).unwrap();
        q.collect().unwrap();
        let v = q.values(0).unwrap();
        assert!(!v.is_empty());
        assert!(v.iter().all(|(_, mhz)| *mhz > 0.0));
    }

    #[test]
    fn unknown_counter_is_an_error() {
        assert!(CounterQuery::new(&[r"\No Such Object(*)\Nothing"]).is_err());
    }
}
