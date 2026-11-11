//! Service configuration collection

use super::issuer::IssuerServiceConfig;
use super::packages::PackagesConfig;
use super::signaling::SignalingConfig;
use serde::{Deserialize, Serialize};

/// Collection of all service configurations
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ServicesConfig {
    /// Optional content-addressed `.actr` artifact hosting.
    #[serde(default)]
    pub packages: Option<PackagesConfig>,

    /// Signer service configuration
    #[serde(default)]
    pub signer: Option<signer::SignerServiceConfig>,

    /// Issuer (Actor Identity Service) configuration
    #[serde(default)]
    pub issuer: Option<IssuerServiceConfig>,

    /// Signaling service configuration
    #[serde(default)]
    pub signaling: Option<SignalingConfig>,
    // Note: STUN/TURN do not depend on Signer, keeping their original configuration approach
}
