use std::process::Command;

use crate::cli::ImportSource;
use crate::config::Config;
use crate::db;
use crate::db::helpers::DbError;
use crate::models::task::ConcurrencyPolicy;
use crate::models::Task;
use crate::systemd::{Systemctl, SystemdManager};

#[derive(Debug)]
pub struct ImportOptions {
    pub source: ImportSource,
    pub include_system: bool,
    pub dry_run: bool,
    pub enable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCandidate {
    pub name: String,
    pub command: String,
    pub schedule: String,
    pub description: String,
}

#[derive(Debug, Default)]
struct ImportSummary {
    found: usize,
    imported: usize,
    skipped: usize,
    failed: usize,
}

pub async fn run_import(options: ImportOptions) -> anyhow::Result<()> {
    let config = Config::load()?;
    let mut candidates = Vec::new();

    if matches!(options.source, ImportSource::All | ImportSource::Crontab) {
        candidates.extend(read_crontab_candidates()?);
    }

    if matches!(options.source, ImportSource::All | ImportSource::Systemd) {
        candidates.extend(read_systemd_candidates(true)?);

        if options.include_system {
            candidates.extend(read_systemd_candidates(false)?);
        }
    }

    let mut summary = ImportSummary {
        found: candidates.len(),
        ..ImportSummary::default()
    };

    if candidates.is_empty() {
        println!("No importable crontab or systemd timer entries found.");
        return Ok(());
    }

    if options.dry_run {
        for candidate in &candidates {
            println!(
                "DRY-RUN import: {} | {} | {}",
                candidate.name, candidate.schedule, candidate.command
            );
        }
        println!("Found {} importable entries.", candidates.len());
        return Ok(());
    }

    let database = db::Database::new(&config.db_path).await?;
    database.run_migrations().await?;
    let conn = database.connect().await?;
    let systemd = if options.enable {
        Some(Systemctl::new(&config)?)
    } else {
        None
    };

    for candidate in candidates {
        let task = Task {
            id: String::new(),
            name: candidate.name.clone(),
            command: candidate.command.clone(),
            schedule: candidate.schedule.clone(),
            tags: vec!["imported".to_string()],
            description: candidate.description.clone(),
            enabled: options.enable,
            max_retries: 0,
            retry_delay_secs: 5,
            timeout_secs: None,
            concurrency_policy: ConcurrencyPolicy::Skip,
            lock_key: None,
            sandbox_profile: None,
            created_at: String::new(),
            updated_at: String::new(),
        };

        match db::tasks::create(&conn, &task).await {
            Ok(created) => {
                if let Some(systemd) = &systemd {
                    if let Err(err) = systemd.install_task(&created).await {
                        summary.failed += 1;
                        eprintln!(
                            "Imported '{}' but failed to install cron-rs timer: {}",
                            created.name, err
                        );
                        continue;
                    }
                }

                summary.imported += 1;
                println!("Imported: {}", created.name);
            }
            Err(DbError::Conflict(_)) => {
                summary.skipped += 1;
                println!("Skipped existing task: {}", candidate.name);
            }
            Err(err) => {
                summary.failed += 1;
                eprintln!("Failed to import '{}': {}", candidate.name, err);
            }
        }
    }

    println!(
        "Import complete: found {}, imported {}, skipped {}, failed {}.",
        summary.found, summary.imported, summary.skipped, summary.failed
    );

    if !options.enable {
        println!("Imported tasks are disabled. Re-run with --enable after disabling originals to avoid duplicate schedules.");
    }

    Ok(())
}

fn read_crontab_candidates() -> anyhow::Result<Vec<ImportCandidate>> {
    let output = Command::new("crontab").arg("-l").output();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            eprintln!("Skipping crontab import: failed to run crontab -l: {err}");
            return Ok(Vec::new());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.to_ascii_lowercase().contains("no crontab") {
            eprintln!("Skipping crontab import: {}", stderr.trim());
        }
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut candidates = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        match parse_crontab_line(line, idx + 1) {
            Ok(Some(candidate)) => candidates.push(candidate),
            Ok(None) => {}
            Err(err) => eprintln!("Skipping crontab line {}: {}", idx + 1, err),
        }
    }
    Ok(candidates)
}

pub fn parse_crontab_line(
    line: &str,
    line_number: usize,
) -> Result<Option<ImportCandidate>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || is_crontab_env_line(trimmed) {
        return Ok(None);
    }

    if trimmed.starts_with('@') {
        let (schedule, command) = split_macro_cron(trimmed)?;
        let schedule = cron_macro_to_on_calendar(schedule)?;
        return Ok(Some(ImportCandidate {
            name: format!("imported-cron-{line_number:03}"),
            command: command.to_string(),
            schedule,
            description: format!("Imported from crontab line {line_number}: {trimmed}"),
        }));
    }

    let (fields, command) = take_fields(trimmed, 5)
        .ok_or_else(|| "expected 5 cron fields followed by a command".to_string())?;
    let schedule = cron_fields_to_on_calendar(&fields)?;

    Ok(Some(ImportCandidate {
        name: format!("imported-cron-{line_number:03}"),
        command: command.to_string(),
        schedule,
        description: format!("Imported from crontab line {line_number}: {trimmed}"),
    }))
}

fn is_crontab_env_line(line: &str) -> bool {
    line.split_whitespace()
        .next()
        .map(|first| first.contains('='))
        .unwrap_or(false)
}

fn split_macro_cron(line: &str) -> Result<(&str, &str), String> {
    let macro_name = line
        .split_whitespace()
        .next()
        .ok_or_else(|| "missing cron macro".to_string())?;
    let command = line[macro_name.len()..].trim();
    if command.is_empty() {
        return Err("missing command after cron macro".to_string());
    }
    Ok((macro_name, command))
}

fn take_fields(line: &str, count: usize) -> Option<(Vec<&str>, &str)> {
    let mut fields = Vec::with_capacity(count);
    let mut rest = line.trim_start();

    for _ in 0..count {
        let mut end = None;
        for (idx, ch) in rest.char_indices() {
            if ch.is_whitespace() {
                end = Some(idx);
                break;
            }
        }

        let end = end?;
        let field = &rest[..end];
        if field.is_empty() {
            return None;
        }
        fields.push(field);
        rest = rest[end..].trim_start();
    }

    if rest.is_empty() {
        None
    } else {
        Some((fields, rest))
    }
}

fn cron_macro_to_on_calendar(macro_name: &str) -> Result<String, String> {
    match macro_name.to_ascii_lowercase().as_str() {
        "@hourly" => Ok("*-*-* *:00:00".to_string()),
        "@daily" | "@midnight" => Ok("*-*-* 00:00:00".to_string()),
        "@weekly" => Ok("Sun *-*-* 00:00:00".to_string()),
        "@monthly" => Ok("*-*-01 00:00:00".to_string()),
        "@yearly" | "@annually" => Ok("*-01-01 00:00:00".to_string()),
        "@reboot" => Err("@reboot has no timer schedule equivalent".to_string()),
        other => Err(format!("unsupported cron macro: {other}")),
    }
}

pub fn cron_fields_to_on_calendar(fields: &[&str]) -> Result<String, String> {
    if fields.len() != 5 {
        return Err("expected exactly 5 cron fields".to_string());
    }

    let minute = normalize_step_origin(fields[0], "0");
    let hour = normalize_step_origin(fields[1], "0");
    let day_of_month = normalize_step_origin(fields[2], "1");
    let month = normalize_named_field(fields[3], &MONTH_NAMES);
    let day_of_week = normalize_day_of_week(fields[4])?;

    if day_of_week.is_some() && day_of_month != "*" {
        return Err("cron lines with both day-of-month and day-of-week cannot be represented as one systemd OnCalendar expression".to_string());
    }

    let date = format!("*-{month}-{day_of_month}");
    let time = format!("{hour}:{minute}:00");

    if let Some(day_of_week) = day_of_week {
        Ok(format!("{day_of_week} {date} {time}"))
    } else {
        Ok(format!("{date} {time}"))
    }
}

fn normalize_step_origin(field: &str, origin: &str) -> String {
    let normalized = normalize_named_field(field, &[]);
    normalized.replace("*/", &format!("{origin}/"))
}

fn normalize_named_field(field: &str, names: &[(&str, &str)]) -> String {
    let mut normalized = field.to_string();
    for (name, value) in names {
        normalized = replace_case_insensitive(&normalized, name, value);
    }
    normalized
}

fn normalize_day_of_week(field: &str) -> Result<Option<String>, String> {
    if field == "*" || field == "?" {
        return Ok(None);
    }

    let field = normalize_named_field(field, &DAY_NAMES);
    let mut parts = Vec::new();
    for token in field.split(',') {
        if token.contains('/') {
            return Err("day-of-week step values are not supported for import".to_string());
        }

        if let Some((start, end)) = token.split_once('-') {
            parts.push(format!(
                "{}..{}",
                day_number_to_name(start)?,
                day_number_to_name(end)?
            ));
        } else {
            parts.push(day_number_to_name(token)?.to_string());
        }
    }

    Ok(Some(parts.join(",")))
}

fn day_number_to_name(value: &str) -> Result<&'static str, String> {
    match value {
        "0" | "7" | "Sun" => Ok("Sun"),
        "1" | "Mon" => Ok("Mon"),
        "2" | "Tue" => Ok("Tue"),
        "3" | "Wed" => Ok("Wed"),
        "4" | "Thu" => Ok("Thu"),
        "5" | "Fri" => Ok("Fri"),
        "6" | "Sat" => Ok("Sat"),
        other => Err(format!("unsupported day-of-week value: {other}")),
    }
}

fn replace_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    let needle_len = needle.len();

    while rest.len() >= needle_len {
        if rest[..needle_len].eq_ignore_ascii_case(needle) {
            output.push_str(replacement);
            rest = &rest[needle_len..];
        } else {
            let ch = rest.chars().next().expect("rest is non-empty");
            output.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }

    output.push_str(rest);
    output
}

const MONTH_NAMES: [(&str, &str); 12] = [
    ("JAN", "1"),
    ("FEB", "2"),
    ("MAR", "3"),
    ("APR", "4"),
    ("MAY", "5"),
    ("JUN", "6"),
    ("JUL", "7"),
    ("AUG", "8"),
    ("SEP", "9"),
    ("OCT", "10"),
    ("NOV", "11"),
    ("DEC", "12"),
];

const DAY_NAMES: [(&str, &str); 7] = [
    ("SUN", "Sun"),
    ("MON", "Mon"),
    ("TUE", "Tue"),
    ("WED", "Wed"),
    ("THU", "Thu"),
    ("FRI", "Fri"),
    ("SAT", "Sat"),
];

fn read_systemd_candidates(user: bool) -> anyhow::Result<Vec<ImportCandidate>> {
    let mut args = Vec::new();
    if user {
        args.push("--user");
    }
    args.extend([
        "list-unit-files",
        "--type=timer",
        "--no-legend",
        "--no-pager",
    ]);

    let output = Command::new("systemctl").args(&args).output();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            eprintln!("Skipping systemd import: failed to run systemctl: {err}");
            return Ok(Vec::new());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Skipping systemd import: {}", stderr.trim());
        return Ok(Vec::new());
    }

    let list = String::from_utf8_lossy(&output.stdout);
    let mut candidates = Vec::new();
    for timer_unit in list.lines().filter_map(timer_unit_from_list_line) {
        if timer_unit.starts_with("cron-rs-") {
            continue;
        }

        match read_systemd_candidate(user, timer_unit) {
            Ok(Some(candidate)) => candidates.push(candidate),
            Ok(None) => {}
            Err(err) => eprintln!("Skipping systemd timer {timer_unit}: {err}"),
        }
    }

    Ok(candidates)
}

fn timer_unit_from_list_line(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .filter(|unit| unit.ends_with(".timer"))
}

fn read_systemd_candidate(user: bool, timer_unit: &str) -> anyhow::Result<Option<ImportCandidate>> {
    let timer_content = systemctl_cat(user, timer_unit)?;
    let timer = parse_timer_unit(timer_unit, &timer_content)?;
    let service_content = systemctl_cat(user, &timer.service_unit)?;
    let command = parse_service_exec_start(&service_content)
        .ok_or_else(|| anyhow::anyhow!("service unit has no ExecStart"))?;

    let scope = if user { "user" } else { "system" };
    Ok(Some(ImportCandidate {
        name: format!(
            "imported-systemd-{scope}-{}",
            sanitize_task_name(timer_unit.trim_end_matches(".timer"))
        ),
        command,
        schedule: timer.schedule,
        description: format!("Imported from {scope} systemd timer {timer_unit}"),
    }))
}

fn systemctl_cat(user: bool, unit: &str) -> anyhow::Result<String> {
    let mut args = Vec::new();
    if user {
        args.push("--user");
    }
    args.extend(["cat", unit]);

    let output = Command::new("systemctl").args(&args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedTimerUnit {
    schedule: String,
    service_unit: String,
}

fn parse_timer_unit(timer_unit: &str, content: &str) -> anyhow::Result<ParsedTimerUnit> {
    let schedule = parse_unit_properties(content, "OnCalendar")
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("timer unit has no OnCalendar"))?;
    let service_unit = parse_last_unit_property(content, "Unit")
        .unwrap_or_else(|| format!("{}.service", timer_unit.trim_end_matches(".timer")));

    Ok(ParsedTimerUnit {
        schedule,
        service_unit,
    })
}

fn parse_service_exec_start(content: &str) -> Option<String> {
    parse_last_unit_property(content, "ExecStart").and_then(|value| {
        let cleaned = value.trim_start_matches(['-', '@', '+', '!', ':']);
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned.to_string())
        }
    })
}

fn parse_last_unit_property(content: &str, key: &str) -> Option<String> {
    let mut value = None;
    for entry in parse_unit_properties(content, key) {
        if entry.is_empty() {
            value = None;
        } else {
            value = Some(entry);
        }
    }
    value
}

fn parse_unit_properties(content: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key}=");
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            line.strip_prefix(&prefix)
                .map(|value| value.trim().to_string())
        })
        .collect()
}

fn sanitize_task_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_simple_crontab_line() {
        let candidate = parse_crontab_line("15 2 * * * /usr/local/bin/backup", 1)
            .unwrap()
            .unwrap();

        assert_eq!(candidate.name, "imported-cron-001");
        assert_eq!(candidate.schedule, "*-*-* 2:15:00");
        assert_eq!(candidate.command, "/usr/local/bin/backup");
    }

    #[test]
    fn converts_cron_macro() {
        let candidate = parse_crontab_line("@daily /usr/local/bin/backup", 3)
            .unwrap()
            .unwrap();

        assert_eq!(candidate.schedule, "*-*-* 00:00:00");
        assert_eq!(candidate.command, "/usr/local/bin/backup");
    }

    #[test]
    fn converts_weekday_range() {
        let schedule = cron_fields_to_on_calendar(&["0", "9", "*", "*", "1-5"]).unwrap();
        assert_eq!(schedule, "Mon..Fri *-*-* 9:0:00");
    }

    #[test]
    fn rejects_cron_dom_and_dow_or_semantics() {
        let err = cron_fields_to_on_calendar(&["0", "9", "1", "*", "1"]).unwrap_err();
        assert!(err.contains("day-of-month and day-of-week"));
    }

    #[test]
    fn skips_env_and_comments() {
        assert!(parse_crontab_line("MAILTO=admin@example.com", 1)
            .unwrap()
            .is_none());
        assert!(parse_crontab_line("# comment", 2).unwrap().is_none());
    }

    #[test]
    fn parses_systemd_timer_and_service_units() {
        let timer = parse_timer_unit(
            "backup.timer",
            "[Timer]\nOnCalendar=*-*-* 02:00:00\nUnit=backup.service\n",
        )
        .unwrap();
        assert_eq!(
            timer,
            ParsedTimerUnit {
                schedule: "*-*-* 02:00:00".to_string(),
                service_unit: "backup.service".to_string(),
            }
        );

        let command =
            parse_service_exec_start("[Service]\nExecStart=/usr/local/bin/backup --all\n").unwrap();
        assert_eq!(command, "/usr/local/bin/backup --all");
    }
}
