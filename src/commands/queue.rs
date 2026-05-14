use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};

use crate::config::queue_db_path;
use crate::queue::{EventStatus, Importance, Queue};

// ---------------------------------------------------------------------------
// Duration parser
// ---------------------------------------------------------------------------

/// Parse a duration string like "7d", "24h", "30m" and return the
/// `DateTime<Utc>` that is that far in the past from now.
pub fn parse_duration_ago(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("duration string is empty"));
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let n: i64 = num_str
        .parse()
        .map_err(|_| anyhow!("invalid duration number in {:?}", s))?;
    let seconds = match unit {
        "d" => n * 86_400,
        "h" => n * 3_600,
        "m" => n * 60,
        "s" => n,
        other => return Err(anyhow!("unknown duration unit {:?} (use d/h/m/s)", other)),
    };
    let now = Utc::now();
    let ago = now - chrono::Duration::seconds(seconds);
    Ok(ago)
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

pub fn list(
    status: Option<String>,
    source: Option<String>,
    importance: Option<String>,
    since: Option<String>,
    limit: u32,
) -> Result<()> {
    let path = queue_db_path();
    let queue = Queue::open(&path)?;

    let status_filter = status.as_deref().map(EventStatus::from_str).transpose()?;
    let importance_filter = importance
        .as_deref()
        .map(Importance::from_str)
        .transpose()?;
    let since_filter = since
        .as_deref()
        .map(parse_duration_ago)
        .transpose()?
        .map(|dt| dt.timestamp());

    let events = queue.list(
        status_filter.as_ref(),
        source.as_deref(),
        importance_filter.as_ref(),
        since_filter,
        Some(limit),
    )?;

    if events.is_empty() {
        println!("No events found.");
        return Ok(());
    }

    // Header
    println!(
        "{:<8} {:<12} {:<10} {:<16} {:<36} CREATED",
        "SEQ", "STATUS", "IMPORTANCE", "SOURCE", "TYPE"
    );
    println!("{}", "-".repeat(100));

    for ev in &events {
        let created = format_unix_ts(ev.created_at);
        println!(
            "{:<8} {:<12} {:<10} {:<16} {:<36} {}",
            ev.seq,
            ev.status,
            ev.importance,
            truncate(&ev.source, 16),
            truncate(&ev.event_type, 36),
            created,
        );
    }

    println!("\n{} event(s) listed.", events.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

pub fn show(seq: i64) -> Result<()> {
    let path = queue_db_path();
    let queue = Queue::open(&path)?;

    let ev = queue
        .get(seq)?
        .ok_or_else(|| anyhow!("Event with seq={} not found", seq))?;

    println!("seq:              {}", ev.seq);
    println!("event_id:         {}", ev.event_id);
    println!("team_id:          {}", ev.team_id);
    println!("source:           {}", ev.source);
    println!("event_type:       {}", ev.event_type);
    println!("importance:       {}", ev.importance);
    println!("status:           {}", ev.status);
    println!(
        "failure_reason:   {}",
        ev.failure_reason.as_deref().unwrap_or("-")
    );
    println!("created_at:       {}", format_unix_ts(ev.created_at));
    println!(
        "enriched_at:      {}",
        ev.enriched_at
            .map(format_unix_ts)
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "delivered_at:     {}",
        ev.delivered_at
            .map(format_unix_ts)
            .unwrap_or_else(|| "-".into())
    );
    println!();
    println!("raw_payload:");
    println!("{}", pretty_json(&ev.raw_payload));

    if let Some(enriched) = &ev.enriched_payload {
        println!();
        println!("enriched_payload:");
        println!("{}", pretty_json(enriched));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// replay
// ---------------------------------------------------------------------------

pub async fn replay(
    status: Option<String>,
    source: Option<String>,
    seq: Option<i64>,
    target: String,
) -> Result<()> {
    let path = queue_db_path();
    let queue = Queue::open(&path)?;

    // Collect events to replay.
    let events = if let Some(s) = seq {
        let ev = queue
            .get(s)?
            .ok_or_else(|| anyhow!("Event with seq={} not found", s))?;
        vec![ev]
    } else {
        // Default to "failed" if no status filter provided.
        let status_str = status.as_deref().unwrap_or("failed");
        let status_filter = EventStatus::from_str(status_str)?;
        let importance_filter: Option<Importance> = None;
        queue.list(
            Some(&status_filter),
            source.as_deref(),
            importance_filter.as_ref(),
            None,
            None,
        )?
    };

    if events.is_empty() {
        eprintln!("No events to replay.");
        return Ok(());
    }

    let client = reqwest::Client::new();
    let mut delivered = 0usize;
    let mut failed = 0usize;

    for ev in &events {
        let body = ev
            .enriched_payload
            .as_deref()
            .unwrap_or(ev.raw_payload.as_str())
            .to_owned();

        let result = client
            .post(&target)
            .header("content-type", "application/json")
            .header("x-kite-source", &ev.source)
            .header("x-kite-event-type", &ev.event_type)
            .header("x-kite-event-id", &ev.event_id)
            .body(body)
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                queue.update_status(ev.seq, &EventStatus::Delivered, None)?;
                println!("#{}: delivered ({})", ev.seq, resp.status());
                delivered += 1;
            }
            Ok(resp) => {
                let reason = format!("HTTP {}", resp.status());
                queue.update_status(ev.seq, &EventStatus::Failed, Some(&reason))?;
                eprintln!("#{}: failed — {}", ev.seq, reason);
                failed += 1;
            }
            Err(e) => {
                let reason = e.to_string();
                queue.update_status(ev.seq, &EventStatus::Failed, Some(&reason))?;
                eprintln!("#{}: failed — {}", ev.seq, reason);
                failed += 1;
            }
        }
    }

    println!(
        "\nReplayed {} event(s): {} delivered, {} failed.",
        events.len(),
        delivered,
        failed
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// flush
// ---------------------------------------------------------------------------

pub fn flush(before: String) -> Result<()> {
    let cutoff = parse_duration_ago(&before)?;
    let before_ts = cutoff.timestamp();

    let path = queue_db_path();
    let queue = Queue::open(&path)?;

    let deleted = queue.flush_before(before_ts)?;
    println!(
        "Flushed {} event(s) created before {}.",
        deleted,
        cutoff.format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------

pub fn stats() -> Result<()> {
    let path = queue_db_path();
    let queue = Queue::open(&path)?;

    let map = queue.stats()?;

    if map.is_empty() {
        println!("Queue is empty.");
        return Ok(());
    }

    println!("{:<16} COUNT", "STATUS");
    println!("{}", "-".repeat(24));

    // Print in a consistent order.
    let order = [
        "pending",
        "enriching",
        "ready",
        "delivered",
        "failed",
        "filtered",
    ];
    let mut total = 0i64;
    for &status in &order {
        if let Some(&count) = map.get(status) {
            println!("{:<16} {}", status, count);
            total += count;
        }
    }
    // Any statuses not in our known list.
    for (status, &count) in &map {
        if !order.contains(&status.as_str()) {
            println!("{:<16} {}", status, count);
            total += count;
        }
    }
    println!("{}", "-".repeat(24));
    println!("{:<16} {}", "TOTAL", total);

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_unix_ts(ts: i64) -> String {
    use chrono::TimeZone;
    match Utc.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => ts.to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

fn pretty_json(s: &str) -> String {
    serde_json::from_str::<serde_json::Value>(s)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| s.to_owned())
}
