use core::cmp::min;
use std::format;
use std::fs::File;
use std::io::{Read, Seek};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::thread::sleep;
use std::time;

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

fn main() -> std::io::Result<()> {
    let (mut previous_receive, mut previous_transmit) = (0, 0);
    let mut file = File::open("/proc/net/dev")?;

    loop {
        let mut file_contents = String::new();
        file.rewind().unwrap();
        file.read_to_string(&mut file_contents).unwrap();
        let mut lines = file_contents.lines();
        let _ = lines.next(); // header line 1
        let _ = lines.next(); // header line 2
        let mut sleep_time = time::Duration::from_secs(1);
        if let Some((receive, transmit)) = lines
            .map(|line| {
                let cols: Vec<_> = line.split_whitespace().collect();
                (
                    cols[0],
                    cols[1].parse::<u64>().unwrap(),
                    cols[9].parse::<u64>().unwrap(),
                )
            })
            .filter(|(iface_raw, _, _)| {
                let iface = iface_raw.strip_suffix(":").unwrap();
                // TODO caching might be faster than syscall
                Path::new(format!("/sys/class/net/{iface}/device").as_str()).exists()
            })
            .map(|(_, receive, transmit)| (receive, transmit))
            .reduce(
                |(previous_recieve, previous_transmit), (receive, transmit)| {
                    (previous_recieve + receive, previous_transmit + transmit)
                },
            )
        {
            let (delta_receive, delta_transmit) =
                (receive - previous_receive, transmit - previous_transmit);

            let mut is_online = true;
            if delta_receive == 0 {
                // possible broken connection
                let start = time::Instant::now();
                let result =
                    ping_rs::send_ping(&PING_ADDR, PING_TIMEOUT, &PING_DATA, Some(&PING_OPTIONS));
                let time_elapsed = start.elapsed();
                if let Err(_) = result {
                    is_online = false;
                }
                sleep_time = sleep_time
                    .checked_sub(time_elapsed)
                    .unwrap_or(time::Duration::ZERO)
            }

            let text = if is_online {
                let receive_txt = human_readable(delta_receive);
                let transmit_txt = human_readable(delta_transmit);
                format!("{:>4}⇣ {:>4}⇡", receive_txt, transmit_txt)
            } else {
                " <span bgcolor=\\\"#eb4d4b\\\"> offline </span> ".to_string()
            };

            println!("{{\"text\": \"{}\"}}", text);

            (previous_receive, previous_transmit) = (receive, transmit);
        }
        sleep(sleep_time);
    }
}
