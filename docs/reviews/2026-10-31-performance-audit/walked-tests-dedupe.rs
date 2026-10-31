use std::{collections::HashSet, hint::black_box, time::Instant};

const MAX_WALK_SEEDS: usize = 200;
const SAMPLE_COUNT: usize = 5;

fn old<'a>(caller_ids: &'a [String], target_id: &str) -> (Vec<&'a str>, Option<(usize, usize)>) {
    let mut callers: Vec<&str> = Vec::new();
    for caller in caller_ids.iter().map(String::as_str) {
        if !caller.is_empty() && caller != target_id && !callers.contains(&caller) {
            callers.push(caller);
        }
    }
    let seeds_cut = (callers.len() > MAX_WALK_SEEDS).then_some((MAX_WALK_SEEDS, callers.len()));
    callers.truncate(MAX_WALK_SEEDS);
    (callers, seeds_cut)
}

fn new<'a>(caller_ids: &'a [String], target_id: &str) -> (Vec<&'a str>, Option<(usize, usize)>) {
    let mut seen = HashSet::new();
    let mut callers: Vec<&str> = Vec::new();
    let mut unique_count = 0;
    for caller in caller_ids.iter().map(String::as_str) {
        if !caller.is_empty() && caller != target_id && seen.insert(caller) {
            unique_count += 1;
            if callers.len() < MAX_WALK_SEEDS {
                callers.push(caller);
            }
        }
    }
    let seeds_cut = (unique_count > MAX_WALK_SEEDS).then_some((MAX_WALK_SEEDS, unique_count));
    (callers, seeds_cut)
}

fn time_old(caller_ids: &[String], repetitions: usize) -> u128 {
    let started = Instant::now();
    for _ in 0..repetitions {
        black_box(old(black_box(caller_ids), "target"));
    }
    started.elapsed().as_nanos() / repetitions as u128
}

fn time_new(caller_ids: &[String], repetitions: usize) -> u128 {
    let started = Instant::now();
    for _ in 0..repetitions {
        black_box(new(black_box(caller_ids), "target"));
    }
    started.elapsed().as_nanos() / repetitions as u128
}

fn percentile_95(samples: &[u128]) -> u128 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[(sorted.len() * 95).div_ceil(100) - 1]
}

fn run_case(label: &str, caller_ids: Vec<String>, repetitions: usize) -> String {
    assert_eq!(old(&caller_ids, "target"), new(&caller_ids, "target"));
    black_box(time_old(&caller_ids, repetitions));
    black_box(time_new(&caller_ids, repetitions));

    let mut samples = Vec::new();
    for sample in 0..SAMPLE_COUNT {
        let old_first = sample % 2 == 0;
        let (old_ns, new_ns) = if old_first {
            (
                time_old(&caller_ids, repetitions),
                time_new(&caller_ids, repetitions),
            )
        } else {
            let new_ns = time_new(&caller_ids, repetitions);
            let old_ns = time_old(&caller_ids, repetitions);
            (old_ns, new_ns)
        };
        samples.push((old_ns, new_ns, old_first));
    }

    let old_samples: Vec<u128> = samples.iter().map(|sample| sample.0).collect();
    let new_samples: Vec<u128> = samples.iter().map(|sample| sample.1).collect();
    let old_mean = old_samples.iter().sum::<u128>() as f64 / SAMPLE_COUNT as f64;
    let new_mean = new_samples.iter().sum::<u128>() as f64 / SAMPLE_COUNT as f64;
    let sample_json = samples
        .iter()
        .enumerate()
        .map(|(index, (old_ns, new_ns, old_first))| {
            format!(
                "{{\"sample\":{},\"order\":\"{}\",\"old_ns_per_operation\":{},\"new_ns_per_operation\":{}}}",
                index + 1,
                if *old_first { "old_first" } else { "new_first" },
                old_ns,
                new_ns
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"workload\":\"{label}\",\"sites\":{},\"unique_callers\":{},\"repetitions_per_sample\":{repetitions},\"warmup_batches_per_arm\":1,\"sample_count_per_arm\":{SAMPLE_COUNT},\"old_mean_ns_per_operation\":{old_mean:.3},\"old_p95_ns_per_operation\":{},\"new_mean_ns_per_operation\":{new_mean:.3},\"new_p95_ns_per_operation\":{},\"mean_speedup\":{:.3},\"samples\":[{sample_json}]}}",
        caller_ids.len(),
        caller_ids
            .iter()
            .filter(|caller| !caller.is_empty() && caller.as_str() != "target")
            .collect::<HashSet<_>>()
            .len(),
        percentile_95(&old_samples),
        percentile_95(&new_samples),
        old_mean / new_mean
    )
}

fn main() {
    let cases: &[(&str, usize, usize)] = &[
        ("unique", 2_000, 30),
        ("half_duplicate", 2_000, 30),
        ("unique", 5_000, 8),
        ("half_duplicate", 5_000, 8),
    ];
    let results = cases
        .iter()
        .map(|&(label, sites, repetitions)| {
            let caller_ids = (0..sites)
                .map(|index| {
                    let unique = if label == "half_duplicate" {
                        index / 2
                    } else {
                        index
                    };
                    format!("caller-{unique}")
                })
                .collect::<Vec<_>>();
            run_case(label, caller_ids, repetitions)
        })
        .collect::<Vec<_>>()
        .join(",\n");
    println!(
        "{{\"benchmark\":\"synthetic eligible-caller-only dedupe loop\",\"site_sizes_are_stress_inputs_not_report_caps\":true,\"results\":[{results}]}}"
    );
}
