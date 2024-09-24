//! Implement traits from [`crate::mgr`] for the circuit types we use.

use crate::build::CircuitBuilder;
use crate::mgr::{self, MockablePlan};
use crate::path::OwnedPath;
use crate::usage::{SupportedCircUsage, TargetCircUsage};
use crate::{timeouts, DirInfo, Error, PathConfig, Result};
use async_trait::async_trait;
use educe::Educe;
use futures::future::OptionFuture;
use std::sync::Arc;
use tor_basic_utils::skip_fmt;
use tor_error::internal;
#[cfg(feature = "vanguards")]
use tor_guardmgr::vanguards::VanguardMgr;
use tor_linkspec::CircTarget;
use tor_proto::circuit::{CircParameters, ClientCirc, Path, UniqId};
use tor_rtcompat::Runtime;

#[async_trait]
impl mgr::AbstractCirc for tor_proto::circuit::ClientCirc {
    type Id = tor_proto::circuit::UniqId;

    fn id(&self) -> Self::Id {
        self.unique_id()
    }

    fn usable(&self) -> bool {
        !self.is_closing()
    }

    fn path_ref(&self) -> Arc<Path> {
        self.path_ref()
    }

    fn n_hops(&self) -> usize {
        self.n_hops()
    }

    fn is_closing(&self) -> bool {
        self.is_closing()
    }

    fn unique_id(&self) -> UniqId {
        self.unique_id()
    }

    async fn extend_ntor<T: CircTarget + std::marker::Sync>(
        &self,
        target: &T,
        params: &CircParameters,
    ) -> tor_proto::Result<()> {
        self.extend_ntor(target, params).await
    }
}

/// The information generated by circuit planning, and used to build a
/// circuit.
#[derive(Educe)]
#[educe(Debug)]
pub(crate) struct Plan {
    /// The supported usage that the circuit will have when complete
    final_spec: SupportedCircUsage,
    /// An owned copy of the path to build.
    // TODO: it would be nice if this weren't owned.
    path: OwnedPath,
    /// The protocol parameters to use when constructing the circuit.
    params: CircParameters,
    /// If this path is using a guard, we'll use this object to report
    /// whether the circuit succeeded or failed.
    guard_status: Option<tor_guardmgr::GuardMonitor>,
    /// If this path is using a guard, we'll use this object to learn
    /// whether we're allowed to use the circuit or whether we have to
    /// wait a while.
    #[educe(Debug(method = "skip_fmt"))]
    guard_usable: Option<tor_guardmgr::GuardUsable>,
}

impl MockablePlan for Plan {}

#[async_trait]
impl<R: Runtime> crate::mgr::AbstractCircBuilder<R> for crate::build::CircuitBuilder<R> {
    type Circ = ClientCirc;
    type Plan = Plan;

    fn plan_circuit(
        &self,
        usage: &TargetCircUsage,
        dir: DirInfo<'_>,
    ) -> Result<(Plan, SupportedCircUsage)> {
        let mut rng = rand::thread_rng();
        let (path, final_spec, guard_status, guard_usable) = usage.build_path(
            &mut rng,
            dir,
            self.guardmgr(),
            #[cfg(all(feature = "vanguards", feature = "hs-common"))]
            self.vanguardmgr(),
            self.path_config().as_ref(),
            self.runtime().wallclock(),
        )?;

        let plan = Plan {
            final_spec: final_spec.clone(),
            path: (&path).try_into()?,
            params: dir.circ_params(),
            guard_status,
            guard_usable,
        };

        Ok((plan, final_spec))
    }

    async fn build_circuit(&self, plan: Plan) -> Result<(SupportedCircUsage, Arc<ClientCirc>)> {
        use crate::build::GuardStatusHandle;
        use tor_guardmgr::GuardStatus;
        let Plan {
            final_spec,
            path,
            params,
            guard_status,
            guard_usable,
        } = plan;

        let guard_usable: OptionFuture<_> = guard_usable.into();
        let guard_status: Arc<GuardStatusHandle> = Arc::new(guard_status.into());

        guard_status.pending(GuardStatus::AttemptAbandoned);

        // TODO: We may want to lower the logic for handling
        // guard_status and guard_usable into build.rs, so that they
        // can be handled correctly on user-selected paths as well.
        //
        // This will probably require a different API for circuit
        // construction.
        match self
            .build_owned(
                path,
                &params,
                Arc::clone(&guard_status),
                final_spec.channel_usage(),
            )
            .await
        {
            Ok(circuit) => {
                // Report success to the guard manager, so it knows that
                // this guard is reachable.
                guard_status.report(GuardStatus::Success);

                // We have to wait for the guard manager to tell us whether
                // this guard is actually _usable_ or not.  Possibly,
                // it is a speculative guard that we're only trying out
                // in case some preferable guard won't meet our needs.
                match guard_usable.await {
                    Some(Ok(true)) | None => (),
                    Some(Ok(false)) => return Err(Error::GuardNotUsable(circuit.unique_id())),
                    Some(Err(_)) => {
                        return Err(internal!("Guard usability status cancelled").into());
                    }
                }
                Ok((final_spec, circuit))
            }
            Err(e) => {
                // The attempt failed; the builder should have set the
                // pending status on the guard to some value which will
                // tell the guard manager whether to blame the guard or not.
                guard_status.commit();

                Err(e)
            }
        }
    }

    fn launch_parallelism(&self, spec: &TargetCircUsage) -> usize {
        match spec {
            TargetCircUsage::Dir => 3,
            _ => 1,
        }
    }

    fn select_parallelism(&self, spec: &TargetCircUsage) -> usize {
        self.launch_parallelism(spec)
    }

    fn learning_timeouts(&self) -> bool {
        CircuitBuilder::learning_timeouts(self)
    }

    fn save_state(&self) -> Result<bool> {
        CircuitBuilder::save_state(self)
    }

    fn path_config(&self) -> Arc<PathConfig> {
        CircuitBuilder::path_config(self)
    }

    fn set_path_config(&self, new_config: PathConfig) {
        CircuitBuilder::set_path_config(self, new_config);
    }

    fn estimator(&self) -> &timeouts::Estimator {
        CircuitBuilder::estimator(self)
    }

    #[cfg(feature = "vanguards")]
    fn vanguardmgr(&self) -> &Arc<VanguardMgr<R>> {
        CircuitBuilder::vanguardmgr(self)
    }

    fn upgrade_to_owned_state(&self) -> Result<()> {
        CircuitBuilder::upgrade_to_owned_state(self)
    }

    fn reload_state(&self) -> Result<()> {
        CircuitBuilder::reload_state(self)
    }

    fn guardmgr(&self) -> &tor_guardmgr::GuardMgr<R> {
        CircuitBuilder::guardmgr(self)
    }

    fn update_network_parameters(&self, p: &tor_netdir::params::NetParameters) {
        CircuitBuilder::update_network_parameters(self, p);
    }
}
