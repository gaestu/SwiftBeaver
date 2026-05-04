use std::collections::{HashMap, HashSet};

use phonenumber::{Mode, country};

const MAX_TRACKED_PHONE_OCCURRENCES: usize = 5_000_000;

#[derive(Debug, Clone)]
pub struct PhoneValidationConfig {
    pub default_region: Option<country::Id>,
    pub supported_regions: Vec<country::Id>,
    national_regions: Vec<country::Id>,
}

impl PhoneValidationConfig {
    pub fn from_regions(default_region: Option<&str>, supported_regions: &[String]) -> Self {
        let default_region = default_region.and_then(parse_region);
        let mut supported = Vec::with_capacity(supported_regions.len());
        for region in supported_regions {
            if let Some(parsed) = parse_region(region)
                && !supported.contains(&parsed)
            {
                supported.push(parsed);
            }
        }
        let mut national_regions = Vec::with_capacity(supported.len() + 1);
        if let Some(region) = default_region {
            national_regions.push(region);
        }
        for region in &supported {
            if !national_regions.contains(region) {
                national_regions.push(*region);
            }
        }

        Self {
            default_region,
            supported_regions: supported,
            national_regions,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedPhone {
    pub raw: String,
    pub e164: String,
    pub country: String,
    pub validation_status: String,
}

#[derive(Debug, Clone, Default)]
pub struct PhoneScanMetrics {
    pub phone_like_spans_scanned: u64,
    pub phone_regex_candidates: u64,
    pub phone_prefilter_rejections: u64,
    pub phone_rejected_digit_only: u64,
    pub phone_rejected_low_entropy: u64,
    pub phone_rejected_bad_context: u64,
    pub phone_rejected_no_region: u64,
    pub phone_rejected_invalid: u64,
    pub phone_validation_calls: u64,
}

impl PhoneScanMetrics {
    pub fn add(&mut self, other: &PhoneScanMetrics) {
        self.phone_like_spans_scanned += other.phone_like_spans_scanned;
        self.phone_regex_candidates += other.phone_regex_candidates;
        self.phone_prefilter_rejections += other.phone_prefilter_rejections;
        self.phone_rejected_digit_only += other.phone_rejected_digit_only;
        self.phone_rejected_low_entropy += other.phone_rejected_low_entropy;
        self.phone_rejected_bad_context += other.phone_rejected_bad_context;
        self.phone_rejected_no_region += other.phone_rejected_no_region;
        self.phone_rejected_invalid += other.phone_rejected_invalid;
        self.phone_validation_calls += other.phone_validation_calls;
    }
}

#[derive(Debug, Clone, Default)]
pub struct PhoneMetricsSnapshot {
    pub phone_like_spans_scanned: u64,
    pub phone_regex_candidates: u64,
    pub phone_prefilter_rejections: u64,
    pub phone_rejected_digit_only: u64,
    pub phone_rejected_low_entropy: u64,
    pub phone_rejected_bad_context: u64,
    pub phone_rejected_no_region: u64,
    pub phone_rejected_invalid: u64,
    pub phone_validation_calls: u64,
    pub phone_validated_rows: u64,
    pub phone_exact_duplicates_omitted: u64,
    pub phone_occurrences_capped: u64,
    pub phone_distinct_normalized_values: u64,
    pub phone_repeated_normalized_values: u64,
}

#[derive(Debug, Clone)]
pub struct PhoneSummaryRow {
    pub normalized_phone: String,
    pub occurrence_count: u64,
    pub first_global_start: u64,
    pub last_global_start: u64,
    pub country: String,
    pub validation_status: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PhoneOccurrenceKey {
    normalized_phone: String,
    global_start: u64,
    global_end: u64,
}

#[derive(Debug, Clone)]
struct PhoneSummaryAccumulator {
    occurrence_count: u64,
    first_global_start: u64,
    last_global_start: u64,
    country: String,
    validation_status: String,
}

#[derive(Debug, Default)]
pub struct PhoneArtefactTracker {
    metrics: PhoneScanMetrics,
    accepted_rows: u64,
    exact_duplicates_omitted: u64,
    occurrences_capped: u64,
    seen_occurrences: HashSet<PhoneOccurrenceKey>,
    summaries: HashMap<String, PhoneSummaryAccumulator>,
}

impl PhoneArtefactTracker {
    pub fn record_scan_metrics(&mut self, metrics: &PhoneScanMetrics) {
        self.metrics.add(metrics);
    }

    pub fn accept_occurrence(
        &mut self,
        normalized_phone: &str,
        country: &str,
        validation_status: &str,
        global_start: u64,
        global_end: u64,
    ) -> bool {
        let key = PhoneOccurrenceKey {
            normalized_phone: normalized_phone.to_string(),
            global_start,
            global_end,
        };
        if self.seen_occurrences.contains(&key) {
            self.exact_duplicates_omitted += 1;
            return false;
        }
        if self.seen_occurrences.len() >= MAX_TRACKED_PHONE_OCCURRENCES {
            self.occurrences_capped += 1;
            return false;
        }

        self.seen_occurrences.insert(key);

        self.accepted_rows += 1;
        self.summaries
            .entry(normalized_phone.to_string())
            .and_modify(|summary| {
                summary.occurrence_count += 1;
                summary.first_global_start = summary.first_global_start.min(global_start);
                summary.last_global_start = summary.last_global_start.max(global_start);
            })
            .or_insert_with(|| PhoneSummaryAccumulator {
                occurrence_count: 1,
                first_global_start: global_start,
                last_global_start: global_start,
                country: country.to_string(),
                validation_status: validation_status.to_string(),
            });
        true
    }

    pub fn metrics_snapshot(&self) -> PhoneMetricsSnapshot {
        let repeated = self
            .summaries
            .values()
            .map(|summary| summary.occurrence_count.saturating_sub(1))
            .sum();
        PhoneMetricsSnapshot {
            phone_like_spans_scanned: self.metrics.phone_like_spans_scanned,
            phone_regex_candidates: self.metrics.phone_regex_candidates,
            phone_prefilter_rejections: self.metrics.phone_prefilter_rejections,
            phone_rejected_digit_only: self.metrics.phone_rejected_digit_only,
            phone_rejected_low_entropy: self.metrics.phone_rejected_low_entropy,
            phone_rejected_bad_context: self.metrics.phone_rejected_bad_context,
            phone_rejected_no_region: self.metrics.phone_rejected_no_region,
            phone_rejected_invalid: self.metrics.phone_rejected_invalid,
            phone_validation_calls: self.metrics.phone_validation_calls,
            phone_validated_rows: self.accepted_rows,
            phone_exact_duplicates_omitted: self.exact_duplicates_omitted,
            phone_occurrences_capped: self.occurrences_capped,
            phone_distinct_normalized_values: self.summaries.len() as u64,
            phone_repeated_normalized_values: repeated,
        }
    }

    pub fn summary_rows(&self) -> Vec<PhoneSummaryRow> {
        let mut rows: Vec<PhoneSummaryRow> = self
            .summaries
            .iter()
            .map(|(normalized_phone, summary)| PhoneSummaryRow {
                normalized_phone: normalized_phone.clone(),
                occurrence_count: summary.occurrence_count,
                first_global_start: summary.first_global_start,
                last_global_start: summary.last_global_start,
                country: summary.country.clone(),
                validation_status: summary.validation_status.clone(),
            })
            .collect();
        rows.sort_by(|a, b| a.normalized_phone.cmp(&b.normalized_phone));
        rows
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneRejectReason {
    DigitOnly,
    LowEntropy,
    BadContext,
    NoRegion,
    Invalid,
}

pub fn validate_candidate(
    raw: &str,
    text: &str,
    start: usize,
    end: usize,
    cfg: &PhoneValidationConfig,
    metrics: &mut PhoneScanMetrics,
) -> Result<ValidatedPhone, PhoneRejectReason> {
    prefilter_candidate(raw, text, start, end)?;

    if has_explicit_international_prefix(raw) {
        metrics.phone_validation_calls += 1;
        return parse_and_validate(None, raw).ok_or(PhoneRejectReason::Invalid);
    }

    if cfg.national_regions.is_empty() {
        return Err(PhoneRejectReason::NoRegion);
    }

    let mut validated: Option<ValidatedPhone> = None;
    for region in &cfg.national_regions {
        metrics.phone_validation_calls += 1;
        if let Some(candidate) = parse_and_validate(Some(*region), raw) {
            if let Some(existing) = &validated {
                if existing.e164 != candidate.e164 {
                    return Err(PhoneRejectReason::Invalid);
                }
            } else {
                validated = Some(candidate);
            }
        }
    }

    validated.ok_or(PhoneRejectReason::Invalid)
}

pub fn record_rejection(metrics: &mut PhoneScanMetrics, reason: PhoneRejectReason) {
    match reason {
        PhoneRejectReason::DigitOnly => {
            metrics.phone_prefilter_rejections += 1;
            metrics.phone_rejected_digit_only += 1;
        }
        PhoneRejectReason::LowEntropy => {
            metrics.phone_prefilter_rejections += 1;
            metrics.phone_rejected_low_entropy += 1;
        }
        PhoneRejectReason::BadContext => {
            metrics.phone_prefilter_rejections += 1;
            metrics.phone_rejected_bad_context += 1;
        }
        PhoneRejectReason::NoRegion => metrics.phone_rejected_no_region += 1,
        PhoneRejectReason::Invalid => metrics.phone_rejected_invalid += 1,
    }
}

fn prefilter_candidate(
    raw: &str,
    text: &str,
    start: usize,
    end: usize,
) -> Result<(), PhoneRejectReason> {
    let digits: Vec<char> = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    let digit_count = digits.len();
    if !(10..=15).contains(&digit_count) {
        return Err(PhoneRejectReason::BadContext);
    }

    if raw.chars().all(|c| c.is_ascii_digit()) {
        return Err(PhoneRejectReason::DigitOnly);
    }

    let unique: HashSet<char> = digits.iter().copied().collect();
    if unique.len() < 4 || has_long_repeated_digit_run(&digits) {
        return Err(PhoneRejectReason::LowEntropy);
    }

    let separator_count = raw.chars().filter(|c| !c.is_ascii_digit()).count();
    if separator_count > digit_count / 2 + 2 {
        return Err(PhoneRejectReason::BadContext);
    }

    if has_bad_boundary(text, start, end) || looks_like_timestamp(raw) {
        return Err(PhoneRejectReason::BadContext);
    }

    Ok(())
}

fn parse_and_validate(region: Option<country::Id>, raw: &str) -> Option<ValidatedPhone> {
    let number = phonenumber::parse(region, raw).ok()?;
    if !phonenumber::is_valid(&number) {
        return None;
    }
    let e164 = number.format().mode(Mode::E164).to_string();
    let country = number
        .country()
        .id()
        .map(|id| id.as_ref().to_string())
        .unwrap_or_else(|| number.country().code().to_string());
    Some(ValidatedPhone {
        raw: raw.to_string(),
        e164,
        country,
        validation_status: "validated".to_string(),
    })
}

fn parse_region(region: &str) -> Option<country::Id> {
    region.trim().to_ascii_uppercase().parse().ok()
}

fn has_explicit_international_prefix(raw: &str) -> bool {
    raw.trim_start().starts_with('+')
}

fn has_long_repeated_digit_run(digits: &[char]) -> bool {
    let mut previous = None;
    let mut run = 0usize;
    for digit in digits {
        if Some(*digit) == previous {
            run += 1;
        } else {
            previous = Some(*digit);
            run = 1;
        }
        if run >= 6 {
            return true;
        }
    }
    false
}

fn has_bad_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char)
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn looks_like_timestamp(raw: &str) -> bool {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 8
        && let (Ok(year), Ok(month), Ok(day)) = (
            digits[0..4].parse::<u32>(),
            digits[4..6].parse::<u32>(),
            digits[6..8].parse::<u32>(),
        )
        && (1970..=2100).contains(&year)
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (raw.contains('-') || raw.contains('/') || raw.contains(':'))
    {
        return true;
    }
    if digits.len() == 14
        && let (Ok(year), Ok(month), Ok(day), Ok(hour), Ok(minute), Ok(second)) = (
            digits[0..4].parse::<u32>(),
            digits[4..6].parse::<u32>(),
            digits[6..8].parse::<u32>(),
            digits[8..10].parse::<u32>(),
            digits[10..12].parse::<u32>(),
            digits[12..14].parse::<u32>(),
        )
    {
        return (1970..=2100).contains(&year)
            && (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour <= 23
            && minute <= 59
            && second <= 59;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_explicit_international_phone() {
        let cfg = PhoneValidationConfig::from_regions(None, &[]);
        let mut metrics = PhoneScanMetrics::default();
        let phone = validate_candidate(
            "+1 650-253-0000",
            "call +1 650-253-0000 now",
            5,
            20,
            &cfg,
            &mut metrics,
        )
        .expect("valid phone");
        assert_eq!(phone.e164, "+16502530000");
        assert_eq!(phone.country, "US");
        assert_eq!(metrics.phone_validation_calls, 1);
    }

    #[test]
    fn rejects_digit_only_by_default() {
        let cfg = PhoneValidationConfig::from_regions(Some("US"), &[]);
        let mut metrics = PhoneScanMetrics::default();
        let err = validate_candidate("6502530000", "6502530000", 0, 10, &cfg, &mut metrics)
            .expect_err("digit-only rejected");
        assert_eq!(err, PhoneRejectReason::DigitOnly);
    }

    #[test]
    fn validates_configured_region_local_phone() {
        let cfg = PhoneValidationConfig::from_regions(Some("US"), &[]);
        let mut metrics = PhoneScanMetrics::default();
        let phone = validate_candidate(
            "650-253-0000",
            "tel 650-253-0000",
            4,
            16,
            &cfg,
            &mut metrics,
        )
        .expect("valid local phone");
        assert_eq!(phone.e164, "+16502530000");
        assert_eq!(phone.country, "US");
    }

    #[test]
    fn rejects_local_phone_without_region() {
        let cfg = PhoneValidationConfig::from_regions(None, &[]);
        let mut metrics = PhoneScanMetrics::default();
        let err = validate_candidate(
            "650-253-0000",
            "tel 650-253-0000",
            4,
            16,
            &cfg,
            &mut metrics,
        )
        .expect_err("region required");
        assert_eq!(err, PhoneRejectReason::NoRegion);
    }

    #[test]
    fn tracker_preserves_offsets_and_omits_exact_duplicates() {
        let mut tracker = PhoneArtefactTracker::default();
        assert!(tracker.accept_occurrence("+14155552671", "US", "validated", 10, 23));
        assert!(tracker.accept_occurrence("+14155552671", "US", "validated", 40, 53));
        assert!(!tracker.accept_occurrence("+14155552671", "US", "validated", 40, 53));
        let metrics = tracker.metrics_snapshot();
        assert_eq!(metrics.phone_validated_rows, 2);
        assert_eq!(metrics.phone_exact_duplicates_omitted, 1);
        assert_eq!(metrics.phone_occurrences_capped, 0);
        assert_eq!(metrics.phone_distinct_normalized_values, 1);
        assert_eq!(metrics.phone_repeated_normalized_values, 1);
        let rows = tracker.summary_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].occurrence_count, 2);
        assert_eq!(rows[0].first_global_start, 10);
        assert_eq!(rows[0].last_global_start, 40);
    }
}
