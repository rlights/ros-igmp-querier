use pnet_datalink::{self, NetworkInterface};
use std::env;
use std::net::Ipv4Addr;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < data.len() {
        let word = if i + 1 < data.len() {
            ((data[i] as u32) << 8) | (data[i + 1] as u32)
        } else {
            (data[i] as u32) << 8
        };
        sum = sum.wrapping_add(word);
        i += 2;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn build_igmp_query(src_mac: [u8; 6], vlan_id: u16, src_ip: Ipv4Addr) -> [u8; 50] {
    let mut pkt = [0u8; 50];
    
    // 1. Ethernet Header (14 bytes)
    // Dest MAC: 01:00:5E:00:00:01
    pkt[0..6].copy_from_slice(&[0x01, 0x00, 0x5e, 0x00, 0x00, 0x01]);
    // Src MAC
    pkt[6..12].copy_from_slice(&src_mac);
    // 802.1Q EtherType
    pkt[12..14].copy_from_slice(&[0x81, 0x00]);
    
    // 2. 802.1Q Header (4 bytes)
    // VLAN ID (Priority 0, CFI 0)
    pkt[14..16].copy_from_slice(&vlan_id.to_be_bytes());
    // IPv4 EtherType
    pkt[16..18].copy_from_slice(&[0x08, 0x00]);
    
    // 3. IPv4 Header (24 bytes, including Router Alert Option)
    pkt[18] = 0x46; // Version 4, IHL 6 (24 bytes)
    pkt[19] = 0xc0; // TOS (Internetwork Control)
    pkt[20..22].copy_from_slice(&32u16.to_be_bytes()); // Total Length 32
    pkt[22..24].copy_from_slice(&0u16.to_be_bytes()); // ID
    pkt[24..26].copy_from_slice(&0u16.to_be_bytes()); // Flags/Frag
    pkt[26] = 1; // TTL 1
    pkt[27] = 2; // Protocol 2 (IGMP)
    // Checksum at 28..30 (initially 0)
    
    let src_ip_octets = src_ip.octets();
    pkt[30..34].copy_from_slice(&src_ip_octets);
    pkt[34..38].copy_from_slice(&[224, 0, 0, 1]); // Dest IP (All Systems)
    
    // Router Alert Option
    pkt[38..42].copy_from_slice(&[0x94, 0x04, 0x00, 0x00]);
    
    // Calculate IPv4 Checksum (over bytes 18..42)
    let ip_csum = checksum(&pkt[18..42]);
    pkt[28..30].copy_from_slice(&ip_csum.to_be_bytes());
    
    // 4. IGMPv2 Header (8 bytes)
    pkt[42] = 0x11; // Type: General Query
    pkt[43] = 100; // Max Resp Time: 10 seconds (in 1/10s)
    // Checksum at 44..46 (initially 0)
    // Group Addr at 46..50 (0.0.0.0 for General Query)
    pkt[46..50].copy_from_slice(&[0, 0, 0, 0]);
    
    // Calculate IGMP Checksum (over bytes 42..50)
    let igmp_csum = checksum(&pkt[42..50]);
    pkt[44..46].copy_from_slice(&igmp_csum.to_be_bytes());
    
    pkt
}

fn main() {
    let interface_name = env::var("INTERFACE").unwrap_or_else(|_| "eth0".to_string());
    let vlans_str = env::var("VLANS").unwrap_or_else(|_| "".to_string());
    let querier_ip_str = env::var("QUERIER_IP").unwrap_or_else(|_| "dynamic".to_string());
    let interval_str = env::var("INTERVAL").unwrap_or_else(|_| "125".to_string());

    let interval = interval_str.parse::<u64>().unwrap_or(125);
    
    let mut vlans: Vec<u16> = Vec::new();
    if !vlans_str.trim().is_empty() {
        for v in vlans_str.split(',') {
            if let Ok(vid) = v.trim().parse::<u16>() {
                if vid > 0 && vid <= 4095 {
                    vlans.push(vid);
                } else {
                    eprintln!("Warning: invalid VLAN ID ignored: {}", vid);
                }
            }
        }
    }

    if vlans.is_empty() {
        eprintln!("Error: No valid VLANs specified. Please set the VLANS env var (e.g. VLANS=10,20).");
        std::process::exit(1);
    }

    let interfaces = pnet_datalink::interfaces();
    let interface = match interfaces.into_iter().find(|iface: &NetworkInterface| iface.name == interface_name) {
        Some(iface) => iface,
        None => {
            eprintln!("Warning: Could not find interface '{}', attempting to auto-detect...", interface_name);
            let mut detected = None;
            for iface in pnet_datalink::interfaces() {
                if !iface.is_loopback() && iface.is_up() && iface.mac.is_some() {
                    detected = Some(iface);
                    break;
                }
            }
            match detected {
                Some(iface) => {
                    eprintln!("Auto-detected interface: {}", iface.name);
                    iface
                },
                None => {
                    eprintln!("Error: Could not auto-detect any valid interfaces.");
                    std::process::exit(1);
                }
            }
        }
    };

    let mac_addr = match interface.mac {
        Some(mac) => mac.octets(),
        None => {
            eprintln!("Error: Interface '{}' does not have a MAC address", interface_name);
            std::process::exit(1);
        }
    };

    println!("Starting IGMP Querier on interface {} (MAC: {:02X?})", interface_name, mac_addr);
    println!("Query Interval: {} seconds", interval);
    println!("Target VLANs: {:?}", vlans);

    let base_ip = if querier_ip_str.to_lowercase() == "dynamic" {
        println!("Using dynamic link-local Querier IPs based on VLAN ID.");
        None
    } else {
        match Ipv4Addr::from_str(&querier_ip_str) {
            Ok(ip) => {
                println!("Using static Querier IP: {}", ip);
                Some(ip)
            },
            Err(_) => {
                eprintln!("Error: Invalid QUERIER_IP format. Must be an IPv4 address or 'dynamic'");
                std::process::exit(1);
            }
        }
    };

    let (mut tx, _rx) = match pnet_datalink::channel(&interface, Default::default()) {
        Ok(pnet_datalink::Channel::Ethernet(tx, rx)) => (tx, rx),
        Ok(_) => {
            eprintln!("Error: Unhandled channel type");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error creating datalink channel: {}", e);
            eprintln!("Make sure you are running as root or have CAP_NET_RAW.");
            std::process::exit(1);
        }
    };

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        println!("Received shutdown signal...");
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    while running.load(Ordering::SeqCst) {
        for vlan_id in &vlans {
            let src_ip = base_ip.unwrap_or_else(|| {
                Ipv4Addr::new(169, 254, (vlan_id >> 8) as u8, (vlan_id & 0xFF) as u8)
            });

            let packet = build_igmp_query(mac_addr, *vlan_id, src_ip);

            match tx.send_to(&packet, None) {
                Some(Ok(())) => {
                    println!("Sent IGMP Query for VLAN {} (Src IP: {})", vlan_id, src_ip);
                }
                Some(Err(e)) => eprintln!("Error sending packet for VLAN {}: {}", vlan_id, e),
                None => eprintln!("Warning: Failed to enqueue packet for VLAN {}", vlan_id),
            }
        }
        
        // Sleep in small increments to allow for graceful shutdown
        for _ in 0..interval {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    println!("IGMP Querier shutdown cleanly.");
}
