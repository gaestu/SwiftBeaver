use crate::metadata::EntropyRegion;

pub fn detect_entropy_regions(
    run_id: &str,
    chunk_start: u64,
    data: &[u8],
    window_size: usize,
    threshold: f64,
) -> Vec<EntropyRegion> {
    if window_size == 0 || data.len() < window_size {
        return Vec::new();
    }

    let mut regions = Vec::new();
    let mut current_start: Option<u64> = None;
    let mut current_end: u64 = 0;
    let mut current_max = 0.0;

    let mut idx = 0usize;
    while idx + window_size <= data.len() {
        let window = &data[idx..idx + window_size];
        let entropy = shannon_entropy(window);
        if entropy >= threshold {
            let win_start = chunk_start + idx as u64;
            let win_end = win_start + window_size as u64 - 1;
            if let Some(start) = current_start {
                if win_start <= current_end + 1 {
                    current_end = win_end;
                    if entropy > current_max {
                        current_max = entropy;
                    }
                } else {
                    regions.push(EntropyRegion {
                        run_id: run_id.to_string(),
                        global_start: start,
                        global_end: current_end,
                        entropy: current_max,
                        window_size: window_size as u64,
                    });
                    current_start = Some(win_start);
                    current_end = win_end;
                    current_max = entropy;
                }
            } else {
                current_start = Some(win_start);
                current_end = win_end;
                current_max = entropy;
            }
        } else if let Some(start) = current_start {
            regions.push(EntropyRegion {
                run_id: run_id.to_string(),
                global_start: start,
                global_end: current_end,
                entropy: current_max,
                window_size: window_size as u64,
            });
            current_start = None;
        }

        idx += window_size;
    }

    if let Some(start) = current_start {
        regions.push(EntropyRegion {
            run_id: run_id.to_string(),
            global_start: start,
            global_end: current_end,
            entropy: current_max,
            window_size: window_size as u64,
        });
    }

    regions
}

fn shannon_entropy(data: &[u8]) -> f64 {
    let mut counts = [0u32; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let len = data.len() as f64;
    let mut entropy = 0.0;
    for count in counts.iter() {
        if *count == 0 {
            continue;
        }
        let p = *count as f64 / len;
        entropy -= p * p.log2();
    }
    entropy
}

#[cfg(test)]
mod tests {
    use super::detect_entropy_regions;

    #[test]
    fn finds_high_entropy_window() {
        let data: Vec<u8> = (0u8..=255u8).collect();
        let regions = detect_entropy_regions("run1", 0, &data, 256, 7.5);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].global_start, 0);
        assert_eq!(regions[0].global_end, 255);
    }

    #[test]
    fn ignores_low_entropy_data() {
        let data = vec![0u8; 1024];
        let regions = detect_entropy_regions("run1", 0, &data, 256, 7.0);
        assert!(regions.is_empty());
    }
}
