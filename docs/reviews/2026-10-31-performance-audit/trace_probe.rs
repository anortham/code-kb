use code_kb_core::{db, ops, queries, reference_report, workspace::Workspace};
use rusqlite::{
    StatementStatus,
    trace::{TraceEvent, TraceEventCodes},
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex, time::Instant};

#[derive(Default)]
struct Sample {
    calls: u64,
    ns: u128,
    steps: i64,
    scans: i64,
    starts: Vec<(i64, i64)>,
}
static SAMPLES: Mutex<BTreeMap<String, Sample>> = Mutex::new(BTreeMap::new());
fn trace(event: TraceEvent<'_>) {
    match event {
        TraceEvent::Stmt(stmt, _) => {
            let mut samples = SAMPLES.lock().unwrap();
            let sample = samples.entry(stmt.sql().into_owned()).or_default();
            sample.calls += 1;
            sample.starts.push((
                stmt.get_status(StatementStatus::VmStep) as i64,
                stmt.get_status(StatementStatus::FullscanStep) as i64,
            ));
        }
        TraceEvent::Profile(stmt, duration) => {
            let mut samples = SAMPLES.lock().unwrap();
            let sample = samples.entry(stmt.sql().into_owned()).or_default();
            let (steps, scans) = sample.starts.pop().unwrap_or_default();
            sample.ns += duration.as_nanos();
            sample.steps += stmt.get_status(StatementStatus::VmStep) as i64 - steps;
            sample.scans += stmt.get_status(StatementStatus::FullscanStep) as i64 - scans;
        }
        _ => {}
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(&args[1]);
    let mode = &args[2];
    let query = args.get(3).map(String::as_str).unwrap_or("");
    let path = args.get(4).filter(|s| !s.is_empty()).map(String::as_str);
    let repeats: usize = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(4);
    let conn = db::open_read_only(&root.join(".code-kb/artifact.db"))?;
    let workspace = Workspace::new(root.clone());
    let target = if ["refs", "context", "tests", "callees", "types"].contains(&mode.as_str()) {
        let id: String = conn.query_row("SELECT symbol_id FROM symbols WHERE name=?1 AND (?2 IS NULL OR path=?2) AND kind NOT IN ('import','export') ORDER BY path,start_line LIMIT 1", rusqlite::params![query,path], |r|r.get(0))?;
        queries::get_symbol_by_id(&conn, &id)?
    } else {
        None
    };
    for run in 0..repeats {
        SAMPLES.lock().unwrap().clear();
        conn.trace_v2(
            TraceEventCodes::SQLITE_TRACE_STMT | TraceEventCodes::SQLITE_TRACE_PROFILE,
            Some(trace),
        );
        let start = Instant::now();
        let result = match mode.as_str() {
            "search" => serde_json::to_value(queries::fts_search_symbols_scoped(
                &conn, query, None, path, false, 20,
            )?)?,
            "lookup" => serde_json::to_value(queries::search_symbols_scoped(
                &conn, query, None, path, false, 20,
            )?)?,
            "outline" => json!(ops::codebase_outline_op(&workspace, &conn, 3, path)?),
            "refs" => serde_json::to_value(reference_report::reference_report(
                &conn,
                &root,
                target.as_ref().unwrap(),
                false,
            )?)?,
            "blast" => serde_json::to_value(ops::blast_radius_op(
                &workspace,
                &conn,
                Some(query),
                path,
                2,
                20,
            )?)?,
            "tests" => serde_json::to_value(queries::find_related_tests(
                &conn,
                target.as_ref().unwrap(),
                5,
            )?)?,
            "callees" => {
                let t = target.as_ref().unwrap();
                serde_json::to_value(queries::find_callee_signatures(
                    &conn,
                    &t.name,
                    &t.symbol_id,
                    10,
                    false,
                )?)?
            }
            "types" => serde_json::to_value(queries::find_related_types(
                &conn,
                target.as_ref().unwrap(),
                10,
            )?)?,
            "context" => {
                let t = target.as_ref().unwrap();
                json!({"callees":queries::find_callee_signatures(&conn,&t.name,&t.symbol_id,10,false)?, "types":queries::find_related_types(&conn,t,10)?, "tests":queries::find_related_tests(&conn,t,5)?})
            }
            _ => panic!("unknown mode"),
        };
        let ms = start.elapsed().as_secs_f64() * 1000.;
        conn.trace_v2(TraceEventCodes::empty(), None);
        let samples = SAMPLES.lock().unwrap();
        let mut entries: Vec<_> = samples.iter().collect();
        entries.sort_by_key(|(_, s)| std::cmp::Reverse(s.steps));
        let queries:Vec<_>=entries.iter().map(|(sql,s)|json!({"calls":s.calls,"sql_ms":s.ns as f64/1e6,"steps":s.steps,"fullscan_steps":s.scans,"sql":sql})).collect();
        println!(
            "{}",
            json!({"root":root,"mode":mode,"query":query,"path":path,"run":run,"warmup":run==0,"wall_ms":ms,"statements":samples.values().map(|s|s.calls).sum::<u64>(),"steps":samples.values().map(|s|s.steps).sum::<i64>(),"fullscan_steps":samples.values().map(|s|s.scans).sum::<i64>(),"queries":queries,"result":result})
        );
    }
    Ok(())
}
