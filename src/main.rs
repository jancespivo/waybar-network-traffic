use core::cmp::min;
use std::format;
use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::str;
use std::thread::sleep;
use std::time;

const NET_DEV_FILE_PATH: &str = "/proc/net/dev";
const SIZE_UNITS: [char; 4] = ['B', 'K', 'M', 'G'];
const PING_DATA: [u8; 1] = [1];
const PING_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
const PING_TIMEOUT: time::Duration = time::Duration::from_secs(1);
const PING_OPTIONS: ping_rs::PingOptions = ping_rs::PingOptions {
    ttl: 128,
    dont_fragment: true,
};

fn human_readable(num_bytes: u64) -> String {
    let idx = min(
        num_bytes.checked_ilog10().unwrap_or(0) / 3,
        SIZE_UNITS.len() as u32 - 1,
    );
    let rounder = 1000_u64.pow(idx);
    let rounded_bytes = (num_bytes as f64 / rounder as f64).round() as u64;
    format!("{}{}", rounded_bytes, SIZE_UNITS[idx as usize])
}

fn parse_iface_stats<F>(contents: &str, mut is_physical_iface: F) -> Option<(u64, u64)>
where
    F: FnMut(&str) -> bool,
{
    contents
        .lines()
        .skip(2)
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let iface_raw = cols.next()?;
            let iface = iface_raw
                .strip_suffix(':')
                .expect("Interface column missing trailing colon");

            if !is_physical_iface(iface) {
                return None;
            }

            let receive = cols
                .next()
                .expect("Missing receive bytes column")
                .parse::<u64>()
                .expect("Invalid receive bytes value");
            let transmit = cols
                .nth(7)
                .expect("Missing transmit bytes column")
                .parse::<u64>()
                .expect("Invalid transmit bytes value");

            Some((receive, transmit))
        })
        .reduce(
            |(previous_receive, previous_transmit), (receive, transmit)| {
                (previous_receive + receive, previous_transmit + transmit)
            },
        )
}

fn main() -> io::Result<()> {
    let (mut previous_receive, mut previous_transmit) = (0_u64, 0_u64);
    let file = File::open(NET_DEV_FILE_PATH)?;
    let mut buffer = [0_u8; 4096];
    let mut stdout = io::stdout().lock();

    loop {
        // Standard read_to_string requires lseek and two read syscalls per iteration.
        // Using read_at at offset 0 reads the procfs entry in a single pread64 syscall without heap allocation.
        let bytes_read = file
            .read_at(&mut buffer, 0)
            .expect("Failed to read /proc/net/dev");
        let file_contents =
            str::from_utf8(&buffer[..bytes_read]).expect("Invalid UTF-8 in /proc/net/dev");

        let mut sleep_time = time::Duration::from_secs(1);
        if let Some((receive, transmit)) = parse_iface_stats(file_contents, |iface| {
            // TODO caching might be faster than syscall
            Path::new(format!("/sys/class/net/{iface}/device").as_str()).exists()
        }) {
            // Interface counters can reset to 0 when a network adapter reconnects or reloads.
            // Using saturating_sub prevents integer underflow panics.
            let (delta_receive, delta_transmit) = (
                receive.saturating_sub(previous_receive),
                transmit.saturating_sub(previous_transmit),
            );

            let mut is_online = true;
            if delta_receive == 0 {
                // possible broken connection
                let start = time::Instant::now();
                let result =
                    ping_rs::send_ping(&PING_ADDR, PING_TIMEOUT, &PING_DATA, Some(&PING_OPTIONS));
                let time_elapsed = start.elapsed();
                if result.is_err() {
                    is_online = false;
                }
                sleep_time = sleep_time
                    .checked_sub(time_elapsed)
                    .unwrap_or(time::Duration::ZERO);
            }

            let write_result = if is_online {
                let receive_txt = human_readable(delta_receive);
                let transmit_txt = human_readable(delta_transmit);
                writeln!(
                    stdout,
                    "{{\"text\": \"{:>4}⇣ {:>4}⇡\"}}",
                    receive_txt, transmit_txt
                )
            } else {
                writeln!(
                    stdout,
                    "{{\"text\": \" <span bgcolor=\\\"#eb4d4b\\\"> offline </span> \"}}"
                )
            };

            if let Err(err) = write_result {
                if err.kind() == ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(err);
            }

            (previous_receive, previous_transmit) = (receive, transmit);
        }
        sleep(sleep_time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_readable() {
        assert_eq!(human_readable(0), "0B");
        assert_eq!(human_readable(999), "999B");
        assert_eq!(human_readable(1000), "1K");
        assert_eq!(human_readable(1500), "2K");
        assert_eq!(human_readable(1_000_000), "1M");
        assert_eq!(human_readable(1_000_000_000), "1G");
    }

    #[test]
    fn test_parse_iface_stats() {
        let sample = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:   26268     294    0    0    0     0          0         0    26268     294    0    0    0     0       0          0
enp1s0f0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0
wlp2s0: 1954093422 1842382    0    0    0     0          0         0 38007237  285665    0   48    0     0       0          0
 wlan0:     226       2    0    0    0     0          0         0      310       2    0    0    0     0       0          0
";
        let stats = parse_iface_stats(sample, |iface| iface != "lo");
        assert_eq!(stats, Some((1_954_093_648, 38_007_547)));
    }
}
