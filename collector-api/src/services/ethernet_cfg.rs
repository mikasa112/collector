//! 有线网口 IPv4 静态配置的参数校验（纯逻辑，不依赖 D-Bus）

use std::net::Ipv4Addr;

use serde::Deserialize;

use crate::services::{ServiceError, ServiceResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ipv4Method {
    /// DHCP
    Auto,
    /// 静态 IP
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticIpv4 {
    pub address: Ipv4Addr,
    pub prefix: u32,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
}

fn invalid(msg: impl Into<String>) -> ServiceError {
    ServiceError::InvalidParameter(msg.into())
}

fn parse_ip(label: &str, s: &str) -> ServiceResult<Ipv4Addr> {
    s.trim()
        .parse::<Ipv4Addr>()
        .map_err(|_| invalid(format!("{label}不是合法的IPv4地址: {s}")))
}

fn mask(prefix: u32) -> u32 {
    u32::MAX << (32 - prefix)
}

/// 校验并解析静态 IPv4 配置。前缀长度限定 1-30，保证网段内有可用主机地址
pub fn validate_static(
    address: &str,
    prefix: u32,
    gateway: Option<&str>,
    dns: &[String],
) -> ServiceResult<StaticIpv4> {
    if !(1..=30).contains(&prefix) {
        return Err(invalid(format!("子网前缀长度须在1-30之间, 收到 {prefix}")));
    }
    let m = mask(prefix);
    let network = |ip: Ipv4Addr| u32::from(ip) & m;
    let broadcast = |ip: Ipv4Addr| u32::from(ip) | !m;

    let address = parse_ip("IP地址", address)?;
    if address.is_loopback() || address.is_unspecified() || address.is_multicast() {
        return Err(invalid(format!("IP地址不可用: {address}")));
    }
    if u32::from(address) == network(address) || u32::from(address) == broadcast(address) {
        return Err(invalid(format!(
            "IP地址 {address} 是该网段的网络地址或广播地址"
        )));
    }

    let gateway = match gateway.map(str::trim).filter(|g| !g.is_empty()) {
        Some(g) => {
            let gw = parse_ip("网关", g)?;
            if gw == address {
                return Err(invalid("网关不能与IP地址相同"));
            }
            if network(gw) != network(address) {
                return Err(invalid(format!(
                    "网关 {gw} 与IP地址 {address} 不在同一网段"
                )));
            }
            if u32::from(gw) == network(gw) || u32::from(gw) == broadcast(gw) {
                return Err(invalid(format!("网关 {gw} 是该网段的网络地址或广播地址")));
            }
            Some(gw)
        }
        None => None,
    };

    if dns.len() > 3 {
        return Err(invalid("DNS最多配置3个"));
    }
    let dns = dns
        .iter()
        .map(|d| {
            let ip = parse_ip("DNS", d)?;
            if ip.is_unspecified() {
                return Err(invalid("DNS不能为0.0.0.0"));
            }
            Ok(ip)
        })
        .collect::<ServiceResult<Vec<_>>>()?;

    Ok(StaticIpv4 {
        address,
        prefix,
        gateway,
        dns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(addr: &str, prefix: u32, gw: Option<&str>) -> ServiceResult<StaticIpv4> {
        validate_static(addr, prefix, gw, &[])
    }

    #[test]
    fn accepts_normal_config() {
        let c = validate_static(
            "192.168.1.10",
            24,
            Some("192.168.1.1"),
            &["114.114.114.114".to_string()],
        )
        .unwrap();
        assert_eq!(c.address, Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(c.gateway, Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(c.dns.len(), 1);
    }

    #[test]
    fn gateway_is_optional_and_blank_means_none() {
        assert_eq!(ok("10.0.0.5", 8, None).unwrap().gateway, None);
        assert_eq!(ok("10.0.0.5", 8, Some(" ")).unwrap().gateway, None);
    }

    #[test]
    fn rejects_bad_prefix() {
        assert!(ok("192.168.1.10", 0, None).is_err());
        assert!(ok("192.168.1.10", 31, None).is_err());
        assert!(ok("192.168.1.10", 33, None).is_err());
    }

    #[test]
    fn rejects_unusable_addresses() {
        assert!(ok("not-an-ip", 24, None).is_err());
        assert!(ok("127.0.0.1", 24, None).is_err());
        assert!(ok("0.0.0.0", 24, None).is_err());
        assert!(ok("224.0.0.1", 24, None).is_err());
        assert!(ok("192.168.1.0", 24, None).is_err()); // 网络地址
        assert!(ok("192.168.1.255", 24, None).is_err()); // 广播地址
    }

    #[test]
    fn rejects_bad_gateway() {
        assert!(ok("192.168.1.10", 24, Some("192.168.2.1")).is_err()); // 不同网段
        assert!(ok("192.168.1.10", 24, Some("192.168.1.10")).is_err()); // 同IP
        assert!(ok("192.168.1.10", 24, Some("192.168.1.255")).is_err());
        assert!(ok("192.168.1.10", 24, Some("abc")).is_err());
    }

    #[test]
    fn rejects_bad_dns() {
        let four: Vec<String> = (1..=4).map(|i| format!("8.8.8.{i}")).collect();
        assert!(validate_static("192.168.1.10", 24, None, &four).is_err());
        assert!(validate_static("192.168.1.10", 24, None, &["0.0.0.0".into()]).is_err());
        assert!(validate_static("192.168.1.10", 24, None, &["x".into()]).is_err());
    }
}
