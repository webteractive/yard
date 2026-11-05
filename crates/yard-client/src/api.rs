//! Every yardd method with its params and answer types, so a call is checked at compile time
//! (M6 design §6.4). Types follow the daemon's handlers; a method that answers nothing worth
//! reading answers `IgnoredAny`, which accepts whatever a newer yardd sends there.

use serde::de::IgnoredAny;
use std::marker::PhantomData;
use yard_proto::methods::*;
use yard_proto::*;

/// A method's name, whether it may take long enough to deserve its own connection, and its
/// params `P` and answer `R`.
pub struct Method<P, R> {
    pub name: &'static str,
    pub slow: bool,
    _t: PhantomData<fn(P) -> R>,
}

impl<P, R> Clone for Method<P, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P, R> Copy for Method<P, R> {}

impl<P, R> Method<P, R> {
    pub const fn quick(name: &'static str) -> Self {
        Method { name, slow: false, _t: PhantomData }
    }

    pub const fn slow(name: &'static str) -> Self {
        Method { name, slow: true, _t: PhantomData }
    }
}

pub const STATUS: Method<(), StatusInfo> = Method::quick(methods::STATUS);
pub const DOCTOR: Method<DoctorParams, DoctorReport> = Method::slow(methods::DOCTOR);
pub const SITES_LIST: Method<(), Vec<SiteInfo>> = Method::quick(methods::SITES_LIST);
pub const SITES_LINK: Method<LinkParams, SiteInfo> = Method::quick(methods::SITES_LINK);
pub const SITES_UNLINK: Method<NameParams, IgnoredAny> = Method::quick(methods::SITES_UNLINK);
pub const SITES_PROXY: Method<ProxyParams, SiteInfo> = Method::quick(methods::SITES_PROXY);
pub const SITES_UNPROXY: Method<NameParams, IgnoredAny> = Method::quick(methods::SITES_UNPROXY);
pub const SITES_DOCROOT: Method<DocrootParams, SiteInfo> = Method::quick(methods::SITES_DOCROOT);
pub const SITES_ISOLATE: Method<IsolateParams, SiteInfo> = Method::quick(methods::SITES_ISOLATE);
pub const SITES_INIT: Method<InitParams, InitResult> = Method::quick(methods::SITES_INIT);
pub const SITES_SECURE: Method<SiteParams, SiteInfo> = Method::quick(methods::SITES_SECURE);
pub const SITES_UNSECURE: Method<SiteParams, SiteInfo> = Method::quick(methods::SITES_UNSECURE);
pub const SITES_INFO: Method<SiteDetailParams, SiteDetail> = Method::quick(methods::SITES_INFO);
pub const CA_INFO: Method<(), CaInfo> = Method::quick(methods::CA_INFO);
pub const PORTS_LIST: Method<(), Vec<PortInfo>> = Method::quick(methods::PORTS_LIST);
pub const PORTS_CHECK: Method<(), Vec<PortInfo>> = Method::slow(methods::PORTS_CHECK);
pub const PORTS_SET: Method<PortSetParams, PortInfo> = Method::quick(methods::PORTS_SET);
pub const PORTS_PIN: Method<UnitParams, PortInfo> = Method::quick(methods::PORTS_PIN);
pub const PORTS_UNPIN: Method<UnitParams, PortInfo> = Method::quick(methods::PORTS_UNPIN);
pub const PORTS_RANGE: Method<RangeParams, IgnoredAny> = Method::quick(methods::PORTS_RANGE);
pub const PORTS_PROFILE: Method<ProfileParams, IgnoredAny> = Method::quick(methods::PORTS_PROFILE);
pub const UNITS_RESTART: Method<UnitParams, IgnoredAny> = Method::quick(methods::UNITS_RESTART);
pub const PHP_LIST: Method<PhpListParams, Vec<PhpInfo>> = Method::quick(methods::PHP_LIST);
pub const PHP_INSTALL: Method<PhpParams, JobStarted> = Method::quick(methods::PHP_INSTALL);
pub const PHP_UNINSTALL: Method<PhpParams, JobStarted> = Method::quick(methods::PHP_UNINSTALL);
pub const PHP_DEFAULT: Method<PhpParams, IgnoredAny> = Method::quick(methods::PHP_DEFAULT);
pub const PHP_CONFIG_GET: Method<PhpConfigParams, Option<String>> = Method::quick(methods::PHP_CONFIG_GET);
pub const PHP_CONFIG_SET: Method<PhpConfigParams, IgnoredAny> = Method::quick(methods::PHP_CONFIG_SET);
pub const PHP_CONFIG_UNSET: Method<PhpConfigParams, bool> = Method::quick(methods::PHP_CONFIG_UNSET);
pub const EXT_ADD: Method<ExtAddParams, JobStarted> = Method::quick(methods::EXT_ADD);
pub const EXT_LIST: Method<ExtListParams, Vec<ExtInfo>> = Method::quick(methods::EXT_LIST);
pub const EXT_ENABLE: Method<ExtParams, IgnoredAny> = Method::quick(methods::EXT_ENABLE);
pub const EXT_DISABLE: Method<ExtParams, IgnoredAny> = Method::quick(methods::EXT_DISABLE);
pub const EXT_REMOVE: Method<ExtParams, JobStarted> = Method::quick(methods::EXT_REMOVE);
pub const XDEBUG_SET: Method<XdebugParams, IgnoredAny> = Method::quick(methods::XDEBUG_SET);
pub const SERVICES_LIST: Method<(), Vec<ServiceInfo>> = Method::quick(methods::SERVICES_LIST);
pub const SERVICES_ADD: Method<ServiceAddParams, JobStarted> = Method::quick(methods::SERVICES_ADD);
pub const SERVICES_REMOVE: Method<ServiceRemoveParams, IgnoredAny> = Method::quick(methods::SERVICES_REMOVE);
pub const SERVICES_START: Method<ServiceParams, ServiceInfo> = Method::quick(methods::SERVICES_START);
pub const SERVICES_STOP: Method<ServiceParams, ServiceInfo> = Method::quick(methods::SERVICES_STOP);
pub const SERVICES_RESTART: Method<ServiceParams, ServiceInfo> = Method::quick(methods::SERVICES_RESTART);
pub const SERVICES_LOGS: Method<ServiceLogsParams, Vec<String>> = Method::slow(methods::SERVICES_LOGS);
pub const SERVICES_PORT: Method<ServicePortParams, ServiceInfo> = Method::quick(methods::SERVICES_PORT);
pub const SERVICES_SLEEP_SET: Method<ServiceSleepParams, ServiceInfo> =
    Method::quick(methods::SERVICES_SLEEP_SET);
pub const ENV_PRINT: Method<EnvParams, Vec<EnvBlock>> = Method::quick(methods::ENV_PRINT);
pub const SOURCES_LIST: Method<(), Vec<SourceInfo>> = Method::quick(methods::SOURCES_LIST);
pub const SOURCES_TRUST: Method<SourceParams, JobStarted> = Method::quick(methods::SOURCES_TRUST);
pub const SOURCES_UNTRUST: Method<SourceParams, JobStarted> = Method::quick(methods::SOURCES_UNTRUST);
pub const NODE_LIST: Method<NodeListParams, NodeList> = Method::quick(methods::NODE_LIST);
/// `node.list` with `available: true`, which asks nodejs.org.
pub const NODE_LIST_AVAILABLE: Method<NodeListParams, NodeList> = Method::slow(methods::NODE_LIST);
pub const NODE_INSTALL: Method<NodeInstallParams, JobStarted> = Method::quick(methods::NODE_INSTALL);
pub const NODE_UNINSTALL: Method<NodeVersionParams, JobStarted> = Method::quick(methods::NODE_UNINSTALL);
pub const NODE_DEFAULT: Method<NodeSpecParams, IgnoredAny> = Method::quick(methods::NODE_DEFAULT);
pub const NODE_USE: Method<NodeUseParams, SiteInfo> = Method::quick(methods::NODE_USE);
pub const NODE_UNUSE: Method<SiteParams, SiteInfo> = Method::quick(methods::NODE_UNUSE);
pub const TUNNELS_LIST: Method<(), Vec<TunnelInfo>> = Method::quick(methods::TUNNELS_LIST);
pub const TUNNELS_DOMAINS: Method<(), Vec<TunnelDomain>> = Method::quick(methods::TUNNELS_DOMAINS);
pub const TUNNELS_DOMAIN_ADD: Method<TunnelDomainAddParams, JobStarted> =
    Method::quick(methods::TUNNELS_DOMAIN_ADD);
pub const TUNNELS_DOMAIN_REMOVE: Method<TunnelDomainParams, Vec<TunnelDomain>> =
    Method::quick(methods::TUNNELS_DOMAIN_REMOVE);
pub const TUNNELS_ADD: Method<TunnelAddParams, JobStarted> = Method::quick(methods::TUNNELS_ADD);
pub const TUNNELS_REMOVE: Method<TunnelRemoveParams, JobStarted> = Method::quick(methods::TUNNELS_REMOVE);
pub const JOBS_CANCEL: Method<JobParams, IgnoredAny> = Method::quick(methods::JOBS_CANCEL);

/// Each entry's method name once (`node.list` has two entries), `hello` and `subscribe` aside.
pub const NAMES: &[&str] = &[
    STATUS.name,
    DOCTOR.name,
    SITES_LIST.name,
    SITES_LINK.name,
    SITES_UNLINK.name,
    SITES_PROXY.name,
    SITES_UNPROXY.name,
    SITES_DOCROOT.name,
    SITES_ISOLATE.name,
    SITES_INIT.name,
    SITES_SECURE.name,
    SITES_UNSECURE.name,
    SITES_INFO.name,
    CA_INFO.name,
    PORTS_LIST.name,
    PORTS_CHECK.name,
    PORTS_SET.name,
    PORTS_PIN.name,
    PORTS_UNPIN.name,
    PORTS_RANGE.name,
    PORTS_PROFILE.name,
    UNITS_RESTART.name,
    PHP_LIST.name,
    PHP_INSTALL.name,
    PHP_UNINSTALL.name,
    PHP_DEFAULT.name,
    PHP_CONFIG_GET.name,
    PHP_CONFIG_SET.name,
    PHP_CONFIG_UNSET.name,
    EXT_ADD.name,
    EXT_LIST.name,
    EXT_ENABLE.name,
    EXT_DISABLE.name,
    EXT_REMOVE.name,
    XDEBUG_SET.name,
    SERVICES_LIST.name,
    SERVICES_ADD.name,
    SERVICES_REMOVE.name,
    SERVICES_START.name,
    SERVICES_STOP.name,
    SERVICES_RESTART.name,
    SERVICES_LOGS.name,
    SERVICES_PORT.name,
    SERVICES_SLEEP_SET.name,
    ENV_PRINT.name,
    SOURCES_LIST.name,
    SOURCES_TRUST.name,
    SOURCES_UNTRUST.name,
    NODE_LIST.name,
    NODE_INSTALL.name,
    NODE_UNINSTALL.name,
    NODE_DEFAULT.name,
    NODE_USE.name,
    NODE_UNUSE.name,
    TUNNELS_LIST.name,
    TUNNELS_DOMAINS.name,
    TUNNELS_DOMAIN_ADD.name,
    TUNNELS_DOMAIN_REMOVE.name,
    TUNNELS_ADD.name,
    TUNNELS_REMOVE.name,
    JOBS_CANCEL.name,
];
