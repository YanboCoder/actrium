//! `actr registry` — remote Actor Registry interactions (Issuer / signaling).
//!
//! Subcommands:
//!   - `discover`    — find services available on the network
//!   - `fingerprint` — compute / verify / lock service semantic fingerprints

use anyhow::Result;
use async_trait::async_trait;
use clap::{Args, Subcommand};

use super::discovery::DiscoveryCommand;
use super::fingerprint::FingerprintCommand;
use crate::core::{Command, CommandContext, CommandResult, ComponentType, ConfigRequirement};

#[derive(Args, Debug)]
pub struct RegistryArgs {
    #[command(subcommand)]
    pub command: RegistryCommand,
}

#[derive(Subcommand, Debug)]
pub enum RegistryCommand {
    /// Discover available Actor services on the network
    Discover(DiscoveryCommand),
    /// Compute / verify / lock service semantic fingerprints
    Fingerprint(FingerprintCommand),
}

#[async_trait]
impl Command for RegistryArgs {
    async fn execute(&self, ctx: &CommandContext) -> Result<CommandResult> {
        match &self.command {
            RegistryCommand::Discover(cmd) => {
                let command = DiscoveryCommand::from_args(cmd);
                {
                    let container = ctx.container.lock().unwrap();
                    container.validate(&command.required_components())?;
                }
                command.execute(ctx).await
            }
            RegistryCommand::Fingerprint(cmd) => cmd.execute(ctx).await,
        }
    }

    fn required_components(&self) -> Vec<ComponentType> {
        match &self.command {
            RegistryCommand::Discover(cmd) => {
                DiscoveryCommand::from_args(cmd).required_components()
            }
            RegistryCommand::Fingerprint(_) => vec![],
        }
    }

    fn config_requirement(&self) -> ConfigRequirement {
        match &self.command {
            // Standalone discover (--endpoint --realm --realm-client-key) needs
            // no local project files; otherwise discover needs manifest.toml or
            // actr.toml to resolve network endpoints.
            RegistryCommand::Discover(cmd) => {
                if cmd.standalone_config().is_some() {
                    ConfigRequirement::None
                } else {
                    ConfigRequirement::RuntimeConfig
                }
            }
            RegistryCommand::Fingerprint(_) => ConfigRequirement::None,
        }
    }

    fn name(&self) -> &str {
        "registry"
    }

    fn description(&self) -> &str {
        "Interact with the remote Actor Registry (discover, fingerprint)"
    }
}
