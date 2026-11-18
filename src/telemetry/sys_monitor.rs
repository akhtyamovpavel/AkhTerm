//! Zero-dependency Linux system monitor reading `/proc` and `/sys/class/power_supply`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const KIB_PER_MIB: f64 = 1024.0;
const KIB_PER_GIB: f64 = 1024.0 * 1024.0;
const MAX_PROC_TREE_NODES: usize = 512;
const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTick {
    pub user: u64, pub nice: u64, pub system: u64, pub idle: u64,
    pub iowait: u64, pub irq: u64, pub softirq: u64, pub steal: u64,
}

impl CpuTick {
    pub fn total(self) -> u64 {
        [self.user, self.nice, self.system, self.idle, self.iowait, self.irq, self.softirq, self.steal]
            .into_iter().fold(0u64, u64::saturating_add)
    }

    pub fn busy(self) -> u64 {
        self.total().saturating_sub(self.idle.saturating_add(self.iowait))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuUsage { pub total_pct: f64, pub user_pct: f64, pub sys_pct: f64, pub iowait_pct: f64 }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadAvg { pub one: f64, pub five: f64, pub fifteen: f64 }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemInfo { pub mem_total_kib: u64, pub mem_available_kib: u64, pub swap_total_kib: u64, pub swap_free_kib: u64 }

impl MemInfo {
    pub fn mem_used_kib(self) -> u64 { self.mem_total_kib.saturating_sub(self.mem_available_kib) }

    pub fn mem_used_pct(self) -> f64 {
        if self.mem_total_kib == 0 { return 0.0; }
        (self.mem_used_kib() as f64 * 100.0 / self.mem_total_kib as f64).clamp(0.0, 100.0)
    }

    pub fn swap_used_kib(self) -> u64 { self.swap_total_kib.saturating_sub(self.swap_free_kib) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatteryState { Discharging, Charging, Full, NotCharging, #[default] Unknown }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BatteryInfo {
    pub capacity_pct: u8, pub state: BatteryState, pub power_watts: Option<f64>,
    pub eta_minutes: Option<u32>, pub health_pct: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel { #[default] Normal, Warn, Hot }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SysSnapshot {
    pub cpu: Option<CpuUsage>, pub load: Option<LoadAvg>, pub mem: Option<MemInfo>,
    pub tab_rss_kib: Option<u64>, pub battery: Option<BatteryInfo>,
}

pub fn parse_proc_stat(content: &str) -> Option<CpuTick> {
    let line = content.lines().find(|l| l.starts_with("cpu "))?;
    let mut vals = line.split_whitespace().skip(1).filter_map(|t| t.parse::<u64>().ok());
    Some(CpuTick {
        user: vals.next()?,
        nice: vals.next()?,
        system: vals.next()?,
        idle: vals.next()?,
        iowait: vals.next().unwrap_or(0),
        irq: vals.next().unwrap_or(0),
        softirq: vals.next().unwrap_or(0),
        steal: vals.next().unwrap_or(0),
    })
}

fn cpu_deltas(prev: CpuTick, curr: CpuTick) -> (u64, u64, u64, u64) {
    let du = curr.user.saturating_sub(prev.user) + curr.nice.saturating_sub(prev.nice);
    let ds = curr.system.saturating_sub(prev.system) + curr.irq.saturating_sub(prev.irq)
        + curr.softirq.saturating_sub(prev.softirq) + curr.steal.saturating_sub(prev.steal);
    (du, ds, curr.idle.saturating_sub(prev.idle), curr.iowait.saturating_sub(prev.iowait))
}

pub fn compute_cpu_usage(prev: CpuTick, curr: CpuTick) -> Option<CpuUsage> {
    if curr.total() <= prev.total() { return None; }
    let (du, ds, di, dw) = cpu_deltas(prev, curr);
    let dt = du.saturating_add(ds).saturating_add(di).saturating_add(dw);
    if dt == 0 { return None; }
    let scale = 100.0 / (dt as f64);
    let busy = curr.busy().saturating_sub(prev.busy()).min(dt);
    let pct = |v: u64| (v as f64 * scale).clamp(0.0, 100.0);
    Some(CpuUsage { total_pct: pct(busy), user_pct: pct(du), sys_pct: pct(ds), iowait_pct: pct(dw) })
}

pub fn parse_loadavg(content: &str) -> Option<LoadAvg> {
    let mut parts = content.split_whitespace();
    Some(LoadAvg {
        one: parts.next()?.parse().ok()?,
        five: parts.next()?.parse().ok()?,
        fifteen: parts.next()?.parse().ok()?,
    })
}

fn parse_kib_field(line: &str) -> Option<u64> {
    line.split_whitespace().nth(1)?.parse::<u64>().ok()
}

#[derive(Default)]
struct MemAcc { total: u64, avail: Option<u64>, free: u64, buf: u64, cached: u64, swap_t: u64, swap_f: u64 }

fn apply_meminfo_line(acc: &mut MemAcc, line: &str) {
    let (Some((key, _)), Some(val)) = (line.split_once(':'), parse_kib_field(line)) else { return };
    match key {
        "MemTotal" => acc.total = val,
        "MemAvailable" => acc.avail = Some(val),
        "MemFree" => acc.free = val,
        "Buffers" => acc.buf = val,
        "Cached" => acc.cached = val,
        "SwapTotal" => acc.swap_t = val,
        "SwapFree" => acc.swap_f = val,
        _ => {}
    }
}

pub fn parse_meminfo(content: &str) -> Option<MemInfo> {
    let mut a = MemAcc::default();
    content.lines().for_each(|l| apply_meminfo_line(&mut a, l));
    if a.total == 0 { return None; }
    let avail = a.avail.unwrap_or_else(|| a.free.saturating_add(a.buf).saturating_add(a.cached));
    Some(MemInfo {
        mem_total_kib: a.total, mem_available_kib: avail.min(a.total),
        swap_total_kib: a.swap_t, swap_free_kib: a.swap_f.min(a.swap_t),
    })
}

pub fn parse_vm_rss_kib(status_content: &str) -> Option<u64> {
    status_content
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(parse_kib_field)
}

fn push_child_pids(pid: i32, queue: &mut Vec<i32>) {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else { return };
    for task in tasks.flatten() {
        if let Ok(raw) = fs::read_to_string(task.path().join("children")) {
            queue.extend(raw.split_whitespace().filter_map(|t| t.parse::<i32>().ok()));
        }
    }
}

fn read_pid_rss_kib(pid: i32) -> Option<u64> {
    parse_vm_rss_kib(&fs::read_to_string(format!("/proc/{pid}/status")).ok()?)
}

pub fn sum_proc_trees_rss_kib(root_pids: &[i32]) -> Option<u64> {
    let mut visited = HashSet::new();
    let mut queue: Vec<i32> = root_pids.iter().copied().filter(|&p| p > 0).collect();
    let (mut total_kib, mut counted) = (0u64, 0usize);
    while let Some(pid) = queue.pop() {
        if pid <= 0 || visited.len() >= MAX_PROC_TREE_NODES || !visited.insert(pid) { continue; }
        if let Some(rss) = read_pid_rss_kib(pid) { total_kib = total_kib.saturating_add(rss); counted += 1; }
        push_child_pids(pid, &mut queue);
    }
    (counted > 0).then_some(total_kib)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn sum_proc_tree_rss_kib(root_pid: i32) -> Option<u64> {
    sum_proc_trees_rss_kib(&[root_pid])
}

fn read_trimmed(dir: &Path, file: &str) -> Option<String> {
    fs::read_to_string(dir.join(file)).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn read_abs_f64(dir: &Path, file: &str) -> Option<f64> {
    read_trimmed(dir, file)?.parse::<f64>().ok().map(f64::abs)
}

fn parse_battery_state(raw: Option<&str>) -> BatteryState {
    match raw.unwrap_or("").to_ascii_lowercase().as_str() {
        "discharging" => BatteryState::Discharging,
        "charging" => BatteryState::Charging,
        "full" => BatteryState::Full,
        "not charging" => BatteryState::NotCharging,
        _ => BatteryState::Unknown,
    }
}

fn is_system_battery(dir: &Path) -> bool {
    let is_bat = read_trimmed(dir, "type").is_some_and(|t| t.eq_ignore_ascii_case("battery"));
    if !is_bat {
        return false;
    }
    let is_device = read_trimmed(dir, "scope").is_some_and(|s| s.eq_ignore_ascii_case("device"));
    let absent = read_trimmed(dir, "present").is_some_and(|p| p == "0");
    !is_device && !absent
}

fn collect_battery_dirs(power_supply_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(power_supply_dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| is_system_battery(p)).collect();
    dirs.sort();
    let bat_only: Vec<PathBuf> = dirs
        .iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_uppercase().starts_with("BAT")))
        .cloned()
        .collect();
    if bat_only.is_empty() { dirs } else { bat_only }
}

struct RawBattery {
    capacity: Option<u8>, state: BatteryState, power_w: Option<f64>,
    energy_now_wh: Option<f64>, energy_full_wh: Option<f64>,
    energy_design_wh: Option<f64>, health_ratio: Option<f64>,
}

fn read_energy_wh(dir: &Path, e_key: &str, c_key: &str, volt: Option<f64>) -> Option<f64> {
    read_abs_f64(dir, e_key)
        .map(|u| u / 1e6)
        .or_else(|| Some(read_abs_f64(dir, c_key)? * volt? / 1e12))
}

fn read_single_battery(dir: &Path) -> RawBattery {
    let volt = read_abs_f64(dir, "voltage_now").filter(|&v| v > 0.0)
        .or_else(|| read_abs_f64(dir, "voltage_min_design").filter(|&v| v > 0.0));
    let power_w = read_energy_wh(dir, "power_now", "current_now", volt);
    let energy_now_wh = read_energy_wh(dir, "energy_now", "charge_now", volt);
    let energy_full_wh = read_energy_wh(dir, "energy_full", "charge_full", volt);
    let energy_design_wh = read_energy_wh(dir, "energy_full_design", "charge_full_design", volt);
    let health_ratio = compute_health_ratio(dir, energy_full_wh, energy_design_wh);
    RawBattery {
        capacity: read_trimmed(dir, "capacity").and_then(|s| s.parse::<u8>().ok()).map(|c| c.min(100)),
        state: parse_battery_state(read_trimmed(dir, "status").as_deref()),
        power_w, energy_now_wh, energy_full_wh, energy_design_wh, health_ratio,
    }
}

fn compute_health_ratio(dir: &Path, full_wh: Option<f64>, design_wh: Option<f64>) -> Option<f64> {
    if let (Some(f), Some(d)) = (full_wh, design_wh) {
        if d > 0.0 { return Some((f / d).clamp(0.0, 1.0)); }
    }
    let cf = read_abs_f64(dir, "charge_full")?;
    let cd = read_abs_f64(dir, "charge_full_design")?;
    (cd > 0.0).then(|| (cf / cd).clamp(0.0, 1.0))
}

fn merge_battery_state(states: &[BatteryState]) -> BatteryState {
    if states.contains(&BatteryState::Discharging) { BatteryState::Discharging }
    else if states.contains(&BatteryState::Charging) { BatteryState::Charging }
    else if states.contains(&BatteryState::Full) { BatteryState::Full }
    else if states.contains(&BatteryState::NotCharging) { BatteryState::NotCharging }
    else { BatteryState::Unknown }
}

fn compute_eta_minutes(state: BatteryState, now_wh: Option<f64>, full_wh: Option<f64>, watts: Option<f64>) -> Option<u32> {
    let power = watts.filter(|&w| w >= 0.05)?;
    let hours = match state {
        BatteryState::Discharging => now_wh.filter(|&e| e > 0.0)? / power,
        BatteryState::Charging => (full_wh? - now_wh?).max(0.0) / power,
        _ => return None,
    };
    let mins = (hours * 60.0).round();
    (mins >= 1.0 && mins <= 5940.0).then_some(mins as u32)
}

fn sum_opt(items: &[RawBattery], pick: impl Fn(&RawBattery) -> Option<f64>) -> Option<f64> {
    let mut found = false;
    let mut total = 0.0;
    for b in items {
        if let Some(v) = pick(b) {
            total += v;
            found = true;
        }
    }
    found.then_some(total)
}

fn combine_capacity(items: &[RawBattery], now_wh: Option<f64>, full_wh: Option<f64>) -> Option<u8> {
    if items.len() == 1 {
        if let Some(c) = items[0].capacity {
            return Some(c);
        }
    }
    if let (Some(n), Some(f)) = (now_wh, full_wh) {
        if f > 0.0 {
            return Some(((n * 100.0 / f).round()).clamp(0.0, 100.0) as u8);
        }
    }
    let caps: Vec<u32> = items.iter().filter_map(|b| b.capacity.map(u32::from)).collect();
    (!caps.is_empty()).then(|| (caps.iter().sum::<u32>() / caps.len() as u32) as u8)
}

fn combined_health_pct(items: &[RawBattery], full_wh: Option<f64>, design_wh: Option<f64>) -> Option<f64> {
    match (full_wh, design_wh) {
        (Some(f), Some(d)) if d > 0.0 => Some(((f / d) * 100.0).clamp(0.0, 100.0)),
        _ => items.iter().find_map(|b| b.health_ratio).map(|r| r * 100.0),
    }
}

pub fn read_battery_from_dir(power_supply_dir: &Path) -> Option<BatteryInfo> {
    let dirs = collect_battery_dirs(power_supply_dir);
    if dirs.is_empty() { return None; }
    let items: Vec<RawBattery> = dirs.iter().map(|d| read_single_battery(d)).collect();
    let state = merge_battery_state(&items.iter().map(|b| b.state).collect::<Vec<_>>());
    let (power_watts, now_wh) = (sum_opt(&items, |b| b.power_w), sum_opt(&items, |b| b.energy_now_wh));
    let (full_wh, design_wh) = (sum_opt(&items, |b| b.energy_full_wh), sum_opt(&items, |b| b.energy_design_wh));
    let capacity_pct = combine_capacity(&items, now_wh, full_wh)?;
    let health_pct = combined_health_pct(&items, full_wh, design_wh);
    let eta_minutes = compute_eta_minutes(state, now_wh, full_wh, power_watts);
    Some(BatteryInfo { capacity_pct, state, power_watts, eta_minutes, health_pct })
}

pub fn read_live_cpu_tick() -> Option<CpuTick> {
    parse_proc_stat(&fs::read_to_string("/proc/stat").ok()?)
}

pub fn read_live_loadavg() -> Option<LoadAvg> {
    parse_loadavg(&fs::read_to_string("/proc/loadavg").ok()?)
}

pub fn read_live_meminfo() -> Option<MemInfo> {
    parse_meminfo(&fs::read_to_string("/proc/meminfo").ok()?)
}

pub fn read_live_battery() -> Option<BatteryInfo> {
    read_battery_from_dir(Path::new(POWER_SUPPLY_DIR))
}

pub struct SysSampler {
    prev_cpu: Option<CpuTick>,
    last_sample: Option<Instant>,
    snapshot: SysSnapshot,
}

impl Default for SysSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl SysSampler {
    pub fn new() -> Self {
        let mut s = Self { prev_cpu: read_live_cpu_tick(), last_sample: None, snapshot: SysSnapshot::default() };
        s.snapshot.load = read_live_loadavg();
        s.snapshot.mem = read_live_meminfo();
        s.snapshot.battery = read_live_battery();
        s
    }

    pub fn snapshot(&self) -> &SysSnapshot {
        &self.snapshot
    }

    pub fn sample_if_due(&mut self, now: Instant, min_interval: Duration, focus_pids: &[i32]) -> bool {
        if self.last_sample.is_some_and(|t| now.saturating_duration_since(t) < min_interval) {
            return false;
        }
        self.sample_at(now, focus_pids);
        true
    }

    pub fn sample_now(&mut self, focus_pids: &[i32]) -> &SysSnapshot {
        self.sample_at(Instant::now(), focus_pids);
        &self.snapshot
    }

    fn sample_at(&mut self, now: Instant, focus_pids: &[i32]) {
        self.last_sample = Some(now);
        if let Some(curr) = read_live_cpu_tick() {
            if let Some(prev) = self.prev_cpu {
                if let Some(usage) = compute_cpu_usage(prev, curr) {
                    self.snapshot.cpu = Some(usage);
                }
            }
            self.prev_cpu = Some(curr);
        }
        self.snapshot.load = read_live_loadavg();
        self.snapshot.mem = read_live_meminfo();
        self.snapshot.tab_rss_kib = sum_proc_trees_rss_kib(focus_pids);
        self.snapshot.battery = read_live_battery();
    }
}

pub fn cpu_alert(cpu: Option<CpuUsage>) -> AlertLevel {
    let Some(c) = cpu else { return AlertLevel::Normal };
    if c.total_pct >= 92.0 { AlertLevel::Hot }
    else if c.total_pct >= 80.0 { AlertLevel::Warn }
    else { AlertLevel::Normal }
}

pub fn ram_alert(mem: Option<MemInfo>) -> AlertLevel {
    let Some(m) = mem else { return AlertLevel::Normal };
    let pct = m.mem_used_pct();
    if pct >= 92.0 { AlertLevel::Hot }
    else if pct >= 85.0 { AlertLevel::Warn }
    else { AlertLevel::Normal }
}

pub fn battery_alert(bat: Option<BatteryInfo>) -> AlertLevel {
    let Some(b) = bat else { return AlertLevel::Normal };
    if b.state != BatteryState::Discharging {
        return AlertLevel::Normal;
    }
    if b.capacity_pct <= 10 { AlertLevel::Hot }
    else if b.capacity_pct <= 20 { AlertLevel::Warn }
    else { AlertLevel::Normal }
}

pub fn alert_level(snap: &SysSnapshot) -> AlertLevel {
    cpu_alert(snap.cpu)
        .max(ram_alert(snap.mem))
        .max(battery_alert(snap.battery))
}

fn format_gib_total(gib: f64) -> String {
    if gib >= 10.0 && (gib - gib.round()).abs() < 0.05 {
        format!("{:.0}", gib.round())
    } else {
        format!("{gib:.1}")
    }
}

pub fn format_ram_compact(mem: MemInfo) -> String {
    let used = mem.mem_used_kib() as f64 / KIB_PER_GIB;
    let total = mem.mem_total_kib as f64 / KIB_PER_GIB;
    format!("{used:.1}/{}G", format_gib_total(total))
}

pub fn format_mem_size(kib: u64) -> String {
    if kib as f64 >= KIB_PER_GIB {
        format!("{:.2} GiB", kib as f64 / KIB_PER_GIB)
    } else if kib as f64 >= KIB_PER_MIB * 10.0 {
        format!("{:.0} MiB", kib as f64 / KIB_PER_MIB)
    } else if kib as f64 >= KIB_PER_MIB {
        format!("{:.1} MiB", kib as f64 / KIB_PER_MIB)
    } else {
        format!("{kib} KiB")
    }
}

pub fn format_eta(minutes: u32, is_ru: bool) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    let (hu, mu) = if is_ru { ("ч", "м") } else { ("h", "m") };
    if h == 0 {
        format!("{m}{mu}")
    } else {
        format!("{h}{hu} {m:02}{mu}")
    }
}

fn format_battery_pill(bat: BatteryInfo) -> String {
    let pct = bat.capacity_pct;
    let watts = bat.power_watts.filter(|&w| w >= 0.1);
    match (bat.state, watts) {
        (BatteryState::Discharging, Some(w)) => format!("{pct}% {w:.1}W"),
        (BatteryState::Charging, Some(w)) => format!("⚡{pct}% {w:.1}W"),
        (BatteryState::Charging, None) => format!("⚡{pct}%"),
        (_, Some(w)) => format!("{pct}% {w:.1}W"),
        (_, None) => format!("{pct}%"),
    }
}

pub fn format_pill_label(snap: &SysSnapshot) -> String {
    let mut parts = Vec::with_capacity(3);
    match snap.cpu {
        Some(c) => parts.push(format!("CPU {:.0}%", c.total_pct)),
        None => parts.push("CPU --%".to_owned()),
    }
    if let Some(m) = snap.mem {
        parts.push(format!("RAM {}", format_ram_compact(m)));
    }
    if let Some(b) = snap.battery {
        parts.push(format_battery_pill(b));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_temp_sysfs(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("akhterm_sysmon_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_kv(dir: &Path, pairs: &[(&str, &str)]) {
        fs::create_dir_all(dir).unwrap();
        for &(k, v) in pairs {
            fs::write(dir.join(k), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn proc_stat_delta_math_covers_zero_half_full_and_wrap_guard() {
        let s0 = parse_proc_stat("cpu  100 0 0 900 0 0 0 0\ncpu0 50 0 0 450 0 0 0 0\n").unwrap();
        let s_idle = parse_proc_stat("cpu  100 0 0 1000 0 0 0 0\n").unwrap();
        let u0 = compute_cpu_usage(s0, s_idle).unwrap();
        assert!((u0.total_pct - 0.0).abs() < 1e-6);

        let s_half = parse_proc_stat("cpu  150 0 0 950 0 0 0 0\n").unwrap();
        let u50 = compute_cpu_usage(s0, s_half).unwrap();
        assert!((u50.total_pct - 50.0).abs() < 1e-6);
        assert!((u50.user_pct - 50.0).abs() < 1e-6);

        let s_full = parse_proc_stat("cpu  160 0 40 900 0 0 0 0\n").unwrap();
        let u100 = compute_cpu_usage(s0, s_full).unwrap();
        assert!((u100.total_pct - 100.0).abs() < 1e-6);
        assert!((u100.sys_pct - 40.0).abs() < 1e-6);

        assert!(compute_cpu_usage(s_full, s0).is_none(), "wrapped/regressed counter must return None");
        assert!(compute_cpu_usage(s0, s0).is_none(), "equal tick must return None");
    }

    #[test]
    fn proc_meminfo_and_loadavg_parse_accurately() {
        let raw = "\
MemTotal:       33554432 kB
MemFree:         4194304 kB
MemAvailable:   18664652 kB
SwapTotal:       8388608 kB
SwapFree:        6291456 kB
";
        let mem = parse_meminfo(raw).unwrap();
        assert_eq!(mem.mem_total_kib, 33_554_432);
        assert_eq!(mem.mem_available_kib, 18_664_652);
        assert_eq!(mem.mem_used_kib(), 14_889_780);
        assert_eq!(mem.swap_total_kib, 8_388_608);
        assert_eq!(mem.swap_used_kib(), 2_097_152);
        assert_eq!(format_ram_compact(mem), "14.2/32G");

        let load = parse_loadavg("1.42 1.18 0.95 2/812 12345\n").unwrap();
        assert!((load.one - 1.42).abs() < 1e-6 && (load.fifteen - 0.95).abs() < 1e-6);
    }

    #[test]
    fn mock_sysfs_discharging_power_now_with_eta_and_health() {
        let root = mk_temp_sysfs("discharging");
        write_kv(&root.join("ADP1"), &[("type", "Mains"), ("online", "0")]);
        write_kv(&root.join("hid-mouse"), &[("type", "Battery"), ("scope", "Device"), ("capacity", "12")]);
        write_kv(&root.join("BAT0"), &[
            ("type", "Battery"), ("status", "Discharging"), ("capacity", "82"),
            ("power_now", "14800000"), ("energy_now", "48100000"),
            ("energy_full", "54000000"), ("energy_full_design", "60000000"),
        ]);
        let bat = read_battery_from_dir(&root).unwrap();
        assert_eq!(bat.capacity_pct, 82);
        assert_eq!(bat.state, BatteryState::Discharging);
        assert!((bat.power_watts.unwrap() - 14.8).abs() < 1e-3);
        assert_eq!(bat.eta_minutes, Some(195));
        assert_eq!(format_eta(bat.eta_minutes.unwrap(), false), "3h 15m");
        assert!((bat.health_pct.unwrap() - 90.0).abs() < 1e-3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mock_sysfs_charging_current_voltage_fallback() {
        let chg = mk_temp_sysfs("charging");
        write_kv(&chg.join("BAT0"), &[
            ("type", "Battery"), ("status", "Charging"), ("capacity", "60"),
            ("current_now", "2000000"), ("voltage_now", "12500000"),
            ("charge_now", "2400000"), ("charge_full", "4000000"), ("charge_full_design", "5000000"),
        ]);
        let bat = read_battery_from_dir(&chg).unwrap();
        assert_eq!(bat.state, BatteryState::Charging);
        assert!((bat.power_watts.unwrap() - 25.0).abs() < 1e-3 && bat.eta_minutes == Some(48));
        assert!((bat.health_pct.unwrap() - 80.0).abs() < 1e-3);
        fs::remove_dir_all(chg).unwrap();
    }

    #[test]
    fn mock_sysfs_full_and_desktop_no_battery() {
        let full = mk_temp_sysfs("full");
        write_kv(&full.join("BAT0"), &[
            ("type", "Battery"), ("status", "Full"), ("capacity", "100"),
            ("power_now", "0"), ("energy_now", "50000000"), ("energy_full", "50000000"),
        ]);
        let bf = read_battery_from_dir(&full).unwrap();
        assert_eq!((bf.state, bf.capacity_pct, bf.eta_minutes), (BatteryState::Full, 100, None));
        fs::remove_dir_all(full).unwrap();

        let desk = mk_temp_sysfs("desktop");
        write_kv(&desk.join("AC"), &[("type", "Mains"), ("online", "1")]);
        assert!(read_battery_from_dir(&desk).is_none());
        fs::remove_dir_all(desk).unwrap();
    }

    fn base_snap() -> SysSnapshot {
        let mem = MemInfo { mem_total_kib: 33_554_432, mem_available_kib: 18_664_652, swap_total_kib: 0, swap_free_kib: 0 };
        let bat = BatteryInfo { capacity_pct: 82, state: BatteryState::Discharging, power_watts: Some(14.8), eta_minutes: Some(195), health_pct: Some(90.0) };
        SysSnapshot { cpu: Some(CpuUsage { total_pct: 18.0, user_pct: 12.0, sys_pct: 6.0, iowait_pct: 0.0 }), load: None, mem: Some(mem), tab_rss_kib: None, battery: Some(bat) }
    }

    #[test]
    fn pill_formatter_matches_compact_format() {
        let snap = base_snap();
        assert_eq!(format_pill_label(&snap), "CPU 18% · RAM 14.2/32G · 82% 14.8W");
        assert_eq!(alert_level(&snap), AlertLevel::Normal);
    }

    #[test]
    fn alert_levels_match_cpu_ram_and_discharging_battery_thresholds() {
        let mut s = base_snap();
        s.cpu = Some(CpuUsage { total_pct: 84.0, user_pct: 70.0, sys_pct: 14.0, iowait_pct: 0.0 });
        assert_eq!(alert_level(&s), AlertLevel::Warn);
        s.cpu = Some(CpuUsage { total_pct: 10.0, user_pct: 5.0, sys_pct: 5.0, iowait_pct: 0.0 });
        s.mem = Some(MemInfo { mem_total_kib: 1000, mem_available_kib: 140, swap_total_kib: 0, swap_free_kib: 0 });
        assert_eq!(alert_level(&s), AlertLevel::Warn);
        s.mem = base_snap().mem;
        s.battery = Some(BatteryInfo { capacity_pct: 20, ..s.battery.unwrap() });
        assert_eq!(alert_level(&s), AlertLevel::Warn);
        s.battery = Some(BatteryInfo { capacity_pct: 9, ..s.battery.unwrap() });
        assert_eq!(alert_level(&s), AlertLevel::Hot);
        s.battery = Some(BatteryInfo { capacity_pct: 9, state: BatteryState::Charging, ..s.battery.unwrap() });
        assert_eq!(alert_level(&s), AlertLevel::Normal);
    }

    #[test]
    fn live_linux_self_read_smoke_test() {
        let tick = read_live_cpu_tick().expect("live /proc/stat");
        assert!(tick.total() > 0);
        let mem = read_live_meminfo().expect("live /proc/meminfo");
        assert!(mem.mem_total_kib > 0 && mem.mem_available_kib <= mem.mem_total_kib);
        let self_pid = std::process::id() as i32;
        let rss = sum_proc_tree_rss_kib(self_pid).expect("own VmRSS");
        assert!(rss > 0);
        let mut sampler = SysSampler::new();
        let snap = sampler.sample_now(&[self_pid]);
        assert!(snap.mem.is_some());
        assert!(snap.tab_rss_kib.unwrap_or(0) > 0);
    }
}
