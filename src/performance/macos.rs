use super::{Process, Reading, Volume, LIST_LEN};
use std::collections::HashMap;
use std::process::Command;
use std::time::Instant;

#[link(name = "System")]
unsafe extern "C" {
    fn mach_host_self() -> u32;
    fn mach_task_self() -> u32;
    fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    fn host_processor_info(
        host: u32,
        flavor: i32,
        processor_count: *mut u32,
        processor_info: *mut *mut i32,
        processor_info_count: *mut u32,
    ) -> i32;
    fn vm_deallocate(target: u32, address: usize, size: usize) -> i32;
}

const HOST_CPU_LOAD_INFO: i32 = 3;
const PROCESSOR_CPU_LOAD_INFO: i32 = 2;
const CPU_STATE_MAX: usize = 4;

#[derive(Clone, Copy, Debug, Default)]
struct CpuTicks([u32; CPU_STATE_MAX]);

fn system_cpu_ticks() -> Option<CpuTicks> {
    let mut ticks = CpuTicks::default();
    let mut count = CPU_STATE_MAX as u32;
    let status = unsafe {
        host_statistics(
            mach_host_self(),
            HOST_CPU_LOAD_INFO,
            ticks.0.as_mut_ptr().cast(),
            &mut count,
        )
    };
    (status == 0 && count as usize >= CPU_STATE_MAX).then_some(ticks)
}

fn core_cpu_ticks() -> Option<Vec<CpuTicks>> {
    let mut processor_count = 0u32;
    let mut info = std::ptr::null_mut();
    let mut count = 0u32;
    let status = unsafe {
        host_processor_info(
            mach_host_self(),
            PROCESSOR_CPU_LOAD_INFO,
            &mut processor_count,
            &mut info,
            &mut count,
        )
    };
    if status != 0 || info.is_null() || processor_count == 0 {
        return None;
    }
    let values = unsafe { std::slice::from_raw_parts(info.cast::<u32>(), count as usize) };
    let mut cores = Vec::with_capacity(processor_count as usize);
    for chunk in values.chunks_exact(CPU_STATE_MAX) {
        cores.push(CpuTicks([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    unsafe {
        let _ = vm_deallocate(
            mach_task_self(),
            info as usize,
            count as usize * std::mem::size_of::<i32>(),
        );
    }
    Some(cores)
}

fn cpu_delta(previous: CpuTicks, current: CpuTicks) -> (f32, Option<f32>) {
    let deltas = std::array::from_fn::<_, CPU_STATE_MAX, _>(|index| {
        current.0[index].wrapping_sub(previous.0[index]) as u64
    });
    let total = deltas.iter().sum::<u64>();
    if total == 0 {
        return (0.0, None);
    }
    let busy = total.saturating_sub(deltas[2]);
    let cpu = busy as f32 * 100.0 / total as f32;
    let kernel = (busy > 0).then_some(deltas[1] as f32 * 100.0 / busy as f32);
    (cpu.clamp(0.0, 100.0), kernel)
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let result = Command::new(program).args(args).output().ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).into_owned())
}

fn scalar(name: &str) -> Option<u64> {
    output("/usr/sbin/sysctl", &["-n", name])?
        .trim()
        .parse()
        .ok()
}

fn cpu_name() -> String {
    output("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"])
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn cpu_core_count() -> usize {
    scalar("hw.ncpu").unwrap_or(1).max(1) as usize
}

fn memory(reading: &mut Reading) {
    let total = scalar("hw.memsize");
    let Some(text) = output("/usr/bin/vm_stat", &[]) else {
        return;
    };
    let page_size = text
        .lines()
        .next()
        .and_then(|line| line.split("page size of ").nth(1))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(4096);
    let page = |name: &str| {
        text.lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.rsplit(':').next())
            .and_then(|tail| tail.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
            * page_size
    };
    let free = page("Pages free:");
    let inactive = page("Pages inactive:");
    let active = page("Pages active:");
    let wired = page("Pages wired down:");
    let compressed = page("Pages occupied by compressor:");
    if let Some(total) = total {
        let used = total.saturating_sub(free + inactive);
        reading.memory = Some((used, total));
    }
    reading.memory_cache = Some(inactive);
    reading.memory_kernel = Some(wired);
    reading.memory_free = Some(free);
    let _ = (active, compressed);
}

fn processes(detail: bool) -> (Vec<Process>, Vec<Process>, Vec<Process>, f32) {
    let Some(text) = output(
        "/bin/ps",
        &["-axo", "pid=,%cpu=,rss=,command="],
    ) else {
        return (Vec::new(), Vec::new(), Vec::new(), 0.0);
    };
    let cores = cpu_core_count() as f32;
    let mut rows: HashMap<String, Process> = HashMap::new();
    let mut total_cpu = 0.0;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|v| v.parse::<usize>().ok()) else {
            continue;
        };
        let cpu = fields.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let rss_kb = fields.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let command = fields.collect::<Vec<_>>().join(" ");
        let executable = command.split_whitespace().next().unwrap_or_default();
        let name = std::path::Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(executable);
        if name.is_empty() {
            continue;
        }
        total_cpu += cpu;
        if detail {
            let entry = rows.entry(name.to_string()).or_insert_with(|| Process {
                name: name.to_string(),
                ..Default::default()
            });
            entry.cpu += cpu / cores;
            entry.memory = entry.memory.saturating_add(rss_kb.saturating_mul(1024));
            if entry.pid == 0 {
                entry.pid = pid;
            }
        }
    }
    let mut by_cpu: Vec<_> = rows.values().cloned().collect();
    by_cpu.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
    by_cpu.truncate(LIST_LEN);
    let mut by_memory: Vec<_> = rows.values().cloned().collect();
    by_memory.sort_by_key(|process| std::cmp::Reverse(process.memory));
    by_memory.truncate(LIST_LEN);
    (by_cpu, by_memory, Vec::new(), total_cpu / cores)
}

fn network() -> HashMap<String, (u64, u64)> {
    let Some(text) = output("/usr/sbin/netstat", &["-ibn"]) else {
        return HashMap::new();
    };
    let mut rows = HashMap::new();
    for line in text.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 10 || fields[0] == "lo0" || fields[0].ends_with('*') {
            continue;
        }
        let Some(received) = fields.get(6).and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        let Some(sent) = fields.get(9).and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        let entry = rows.entry(fields[0].trim_end_matches('*').to_string()).or_insert((0, 0));
        entry.0 = entry.0.max(received);
        entry.1 = entry.1.max(sent);
    }
    rows
}

fn volumes() -> Vec<Volume> {
    let Some(text) = output("/bin/df", &["-kP"]) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 6 || !fields[0].starts_with("/dev/") {
                return None;
            }
            let total = fields[1].parse::<u64>().ok()?.saturating_mul(1024);
            let used = fields[2].parse::<u64>().ok()?.saturating_mul(1024);
            let mount = fields[5..].join(" ");
            let label = if mount == "/" {
                "Macintosh HD".to_string()
            } else {
                std::path::Path::new(&mount)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&mount)
                    .to_string()
            };
            if !seen.insert(label.clone()) || total == 0 {
                return None;
            }
            Some(Volume {
                letter: label.chars().next().unwrap_or('M'),
                used,
                total,
            })
        })
        .collect()
}

fn disk_counters() -> Option<(u64, u64)> {
    let text = output(
        "/usr/sbin/ioreg",
        &["-r", "-c", "IOBlockStorageDriver", "-k", "Statistics", "-l"],
    )?;
    let number = |line: &str, key: &str| {
        let start = line.find(&format!("\"{key}\"="))? + key.len() + 3;
        line[start..]
            .split(|ch: char| !ch.is_ascii_digit())
            .next()
            .and_then(|value| value.parse::<u64>().ok())
    };
    let mut read = 0u64;
    let mut write = 0u64;
    let mut found = false;
    for line in text.lines() {
        let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
        if indent != 2 || !line.contains("\"Statistics\"") {
            continue;
        }
        if let Some(value) = number(line, "Bytes (Read)") {
            read = read.saturating_add(value);
            found = true;
        }
        if let Some(value) = number(line, "Bytes (Write)") {
            write = write.saturating_add(value);
            found = true;
        }
    }
    found.then_some((read, write))
}

fn gpu_identity(text: &str) -> Option<(String, Option<u32>)> {
    let string_value = |line: &str, key: &str| -> Option<String> {
        let start = line.find(&format!("\"{key}\" = \""))? + key.len() + 5;
        let tail = &line[start..];
        let end = tail.find('"')?;
        let value = tail[..end].trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    let number = |line: &str, key: &str| {
        let key = format!("\"{key}\"");
        let start = line.find(&key)? + key.len();
        let tail = line[start..].trim_start();
        let tail = tail.strip_prefix('=')?.trim_start();
        tail
            .trim_start_matches('"')
            .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .next()
            .and_then(|value| value.parse::<f32>().ok())
    };
    let mut accelerator = false;
    let mut model = None;
    let mut cores = None;
    for line in text.lines() {
        if line.contains("<class ") && line.contains("Accelerator") {
            accelerator = true;
        }
        if model.is_none() {
            model = string_value(line, "model");
        }
        if cores.is_none() {
            cores = number(line, "gpu-core-count").map(|value| value.max(0.0) as u32);
        }
    }
    if !accelerator {
        return None;
    }
    let name = model
        .or_else(|| {
            let cpu = cpu_name();
            (!cpu.is_empty()).then_some(cpu)
        })
        .unwrap_or_else(|| "Apple".into());
    let mut gpu_name = if name.to_ascii_lowercase().contains("gpu")
        || name.to_ascii_lowercase().contains("graphics")
    {
        name
    } else {
        format!("{name} GPU")
    };
    let cores = cores.filter(|cores| *cores > 0);
    if let Some(cores) = cores {
        gpu_name.push_str(&format!(" · {cores} 核"));
    }
    Some((gpu_name, cores))
}

fn gpu(reading: &mut Reading) {
    if reading.gpu_available != Some(true) {
        reading.gpu_available = Some(false);
    }
    let Some(text) = output(
        "/usr/sbin/ioreg",
        &["-r", "-d", "1", "-c", "IOAccelerator", "-k", "PerformanceStatistics"],
    ) else {
        return;
    };
    let number = |line: &str, key: &str| {
        let key = format!("\"{key}\"");
        let start = line.find(&key)? + key.len();
        let tail = line[start..].trim_start();
        let tail = tail.strip_prefix('=')?.trim_start();
        tail
            .trim_start_matches('"')
            .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
            .next()
            .and_then(|value| value.parse::<f32>().ok())
    };
    let mut utilization = None;
    let mut shared_used = None;
    for line in text.lines().filter(|line| line.contains("\"PerformanceStatistics\"")) {
        for key in ["Device Utilization %", "Renderer Utilization %", "Tiler Utilization %"] {
            if let Some(value) = number(line, key).filter(|value| value.is_finite()) {
                utilization = Some(utilization.map_or(value, |current: f32| current.max(value)));
            }
        }
        if let Some(value) = number(line, "In use system memory").filter(|value| *value >= 0.0) {
            shared_used = Some(shared_used.map_or(value, |current: f32| current.max(value)));
        }
    }
    let Some((gpu_name, cores)) = gpu_identity(&text) else { return; };
    reading.gpu_available = Some(true);
    reading.gpu_cores = cores;
    if reading.gpu.is_none() {
        reading.gpu = utilization.map(|value| value.clamp(0.0, 100.0));
    }
    reading.gpu_name = gpu_name;
    if let (Some(used), Some(total)) = (shared_used, scalar("hw.memsize")) {
        reading.shared_memory = Some((used as u64, total));
    }
}

pub(super) struct Sampler {
    network: Option<(Instant, HashMap<String, (u64, u64)>)>,
    disk: Option<(Instant, (u64, u64))>,
    cpu: Option<CpuTicks>,
    cores: Option<Vec<CpuTicks>>,
    enhanced: Option<macmon::Sampler>,
    enhanced_retry: Option<Instant>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            network: None,
            disk: None,
            cpu: None,
            cores: None,
            enhanced: macmon::Sampler::new().ok(),
            enhanced_retry: None,
        }
    }
}

impl Sampler {
    pub fn read(&mut self, now: Instant, detail: bool) -> Reading {
        let mut reading = Reading::default();
        let (by_cpu, by_memory, by_io, ps_cpu) = processes(detail);
        reading.by_cpu = by_cpu;
        reading.by_memory = by_memory;
        reading.by_io = by_io;
        memory(&mut reading);

        let system_ticks = system_cpu_ticks();
        if let (Some(previous), Some(current)) = (self.cpu, system_ticks) {
            let (cpu, kernel) = cpu_delta(previous, current);
            reading.cpu = Some(cpu);
            reading.cpu_kernel = kernel;
        }
        self.cpu = system_ticks;
        let current_cores = core_cpu_ticks();
        if let (Some(previous), Some(current)) = (&self.cores, &current_cores) {
            if previous.len() == current.len() {
                reading.cores = previous
                    .iter()
                    .zip(current)
                    .map(|(&before, &after)| cpu_delta(before, after).0)
                    .collect();
            }
        }
        let cores = current_cores
            .as_ref()
            .map_or_else(cpu_core_count, Vec::len)
            .max(1);
        self.cores = current_cores;
        if reading.cores.len() != cores {
            reading.cores = vec![reading.cpu.unwrap_or(ps_cpu); cores];
        }
        reading.cpu.get_or_insert(ps_cpu.clamp(0.0, 100.0));
        if self.enhanced.is_none()
            && self.enhanced_retry.is_none_or(|retry| now >= retry)
        {
            self.enhanced = macmon::Sampler::new().ok();
            self.enhanced_retry = Some(now + std::time::Duration::from_secs(30));
        }
        if let Some(enhanced) = &mut self.enhanced {
            match enhanced.get_metrics_now() {
                Ok(metrics) => {
                    reading.gpu_available = Some(true);
                    reading.gpu = Some((metrics.gpu_scaled_ratio * 100.0).clamp(0.0, 100.0));
                    if metrics.gpu_freq_mhz > 0 {
                        reading.gpu_mhz = Some(metrics.gpu_freq_mhz as f32);
                    }
                    reading.gpu_temp = metrics.temp.gpu_temp_avg;
                }
                Err(_) => {
                    self.enhanced = None;
                    self.enhanced_retry = Some(now + std::time::Duration::from_secs(30));
                }
            }
        }
        gpu(&mut reading);
        if reading.gpu_available == Some(true) && reading.gpu_name.is_empty() {
            let name = cpu_name();
            reading.gpu_name = if name.is_empty() {
                "Apple GPU".into()
            } else {
                format!("{name} GPU")
            };
        }
        reading.gpu_engines = Vec::new();

        let current = network();
        if let Some((at, previous)) = &self.network {
            let seconds = now.duration_since(*at).as_secs_f64().max(0.001);
            let sum = |slot: usize| -> Option<f32> {
                let bytes = current.iter().try_fold(0u64, |total, (name, values)| {
                    let value = if slot == 0 { values.0 } else { values.1 };
                    let old = previous
                        .get(name)
                        .map_or(value, |old| if slot == 0 { old.0 } else { old.1 });
                    Some(total.saturating_add(value.saturating_sub(old)))
                })?;
                Some(bytes as f32 / seconds as f32)
            };
            reading.network = [sum(0), sum(1)];
        }
        reading.net_total = Some(
            current
                .values()
                .fold([0u64; 2], |total, value| {
                    [total[0].saturating_add(value.0), total[1].saturating_add(value.1)]
                }),
        );
        self.network = Some((now, current));

        let current_disk = disk_counters();
        if let (Some((at, previous)), Some(current)) = (&self.disk, current_disk) {
            let seconds = now.duration_since(*at).as_secs_f64().max(0.001);
            reading.disk = [
                Some(current.0.saturating_sub(previous.0) as f32 / seconds as f32),
                Some(current.1.saturating_sub(previous.1) as f32 / seconds as f32),
            ];
        }
        self.disk = current_disk.map(|counters| (now, counters));

        reading.volumes = volumes();
        reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_identity_requires_an_accelerator_and_reports_core_count() {
        assert!(gpu_identity("").is_none());
        let accelerator = r#"
            +-o AGXAcceleratorG18G <class AGXAcceleratorG18G, id 1>
              {
                "model" = "Apple M6"
                "gpu-core-count" = 12
              }
        "#;
        assert_eq!(
            gpu_identity(accelerator),
            Some(("Apple M6 GPU · 12 核".into(), Some(12)))
        );
    }

    #[test]
    fn macos_system_metrics_are_available_without_opening_process_details() {
        let mut sampler = Sampler::default();
        let first = sampler.read(Instant::now(), false);
        assert!(first.memory.is_some(), "内存总量与已用量应可读取");
        assert!(!first.volumes.is_empty(), "至少应识别系统磁盘");

        std::thread::sleep(std::time::Duration::from_millis(1_050));
        let second = sampler.read(Instant::now(), false);
        let cpu = second.cpu.expect("系统总 CPU 不应依赖展开页进程排行");
        assert!((0.0..=100.0).contains(&cpu));
        assert!(!second.cores.is_empty(), "应读取逻辑处理器负载");
        assert!(second.disk[0].is_some() && second.disk[1].is_some(), "磁盘读写应可读取");
        assert!(second.network[0].is_some() && second.network[1].is_some(), "网络收发应可读取");
        assert_eq!(second.gpu_available, Some(true), "应识别到 GPU 硬件");
        assert!(second.gpu.is_some(), "应读取 GPU 利用率");
        assert!(second.gpu_name.contains("GPU"), "GPU 名称应显示为图形处理器");
        println!(
            "gpu={} cores={:?} usage={:?} freq={:?}",
            second.gpu_name, second.gpu_cores, second.gpu, second.gpu_mhz
        );
        assert!(second.by_cpu.is_empty(), "未打开详情时不应构建进程排行");
    }
}
