//! Measurements for AC-3: sizes and times of MLS operations at 2, 10, 100 and 1,000 members, natively and in WebAssembly.
//!
//! The harness drives the clients and the server's validator directly rather than through [`crate::sim`], so that a large group needs only a few devices to really join: every member is a real device with a real key package in the ratchet tree, but only the owner and up to two more members process messages. Their numbers are the ones a real member would see.
//!
//! Two tree shapes matter (REPORT.md explains them):
//!
//! - **After a bulk add** (one commit adding everyone, which is also what group re-creation produces): most of the tree's inner nodes are blank, so each member's first commit must encrypt to almost every other member. This is MLS's worst case and is what the `update`, `add_one` and `remove_one` rows measure.
//! - **Healed** (every member has committed once): each commit encrypts to about log2(n) nodes. The `healed` rows measure this for groups up to [`BenchConfig::healed_up_to`] members, where making every member join and commit is affordable.

use std::collections::BTreeMap;

use web_time::Instant;

use crate::client::{Change, Client, ClientError};
use crate::directory::KeyPackageDirectory;
use crate::identity::DeviceId;
use crate::roster::Role;
use crate::suite::Suite;
use crate::update::{Update, UpdateKind};
use crate::validator::{Rejection, Validator};

/// What to measure.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// The ciphersuites.
    pub suites: Vec<Suite>,
    /// The group sizes.
    pub members: Vec<usize>,
    /// The update payload sizes, in bytes.
    pub payloads: Vec<usize>,
    /// How often each commit measurement is repeated (the median is reported).
    pub repetitions: usize,
    /// The largest group in which every member joins and commits once, for the healed-tree measurement.
    pub healed_up_to: usize,
}

impl BenchConfig {
    /// The measurements of the report: every suite of this build at 2, 10, 100 and 1,000 members, payloads of 100 B to 100 KB.
    #[must_use]
    pub fn full() -> Self {
        Self {
            suites: Suite::all()
                .iter()
                .copied()
                .filter(|suite| *suite != Suite::ClassicalChaCha)
                .collect(),
            members: vec![2, 10, 100, 1_000],
            payloads: vec![100, 1_000, 10_000, 100_000],
            repetitions: 5,
            healed_up_to: 100,
        }
    }

    /// A quick run that checks the harness works (part of the test suite).
    #[must_use]
    pub fn smoke() -> Self {
        Self {
            suites: vec![Suite::Classical],
            members: vec![2, 10],
            payloads: vec![100],
            repetitions: 1,
            healed_up_to: 10,
        }
    }
}

/// One measured value.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// The ciphersuite.
    pub suite: Suite,
    /// The group size.
    pub members: usize,
    /// What was measured, for example `update.commit` or `message.100000.decrypt`.
    pub metric: String,
    /// The value.
    pub value: f64,
    /// `bytes` or `ms`.
    pub unit: &'static str,
}

impl Measurement {
    /// One Markdown table row: suite, members, metric, value, unit.
    #[must_use]
    pub fn row(&self) -> String {
        let value = if self.unit == "bytes" {
            format!("{:.0}", self.value)
        } else {
            format!("{:.3}", self.value)
        };
        format!(
            "| {} | {} | {} | {} | {} |",
            self.suite.label(),
            self.members,
            self.metric,
            value,
            self.unit
        )
    }
}

/// Why a measurement run failed.
#[derive(Debug, thiserror::Error)]
#[error("benchmark failed: {0}")]
pub struct BenchError(String);

impl From<ClientError> for BenchError {
    fn from(error: ClientError) -> Self {
        Self(error.to_string())
    }
}

impl From<Rejection> for BenchError {
    fn from(error: Rejection) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::identity::IdentityError> for BenchError {
    fn from(error: crate::identity::IdentityError) -> Self {
        Self(error.to_string())
    }
}

/// Runs `f` and returns its result with the elapsed time in milliseconds.
fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let result = f();
    (result, start.elapsed().as_secs_f64() * 1000.0)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    match values.len() {
        0 => f64::NAN,
        length if length % 2 == 1 => values[length / 2],
        length => (values[length / 2 - 1] + values[length / 2]) / 2.0,
    }
}

fn bytes(length: usize) -> f64 {
    // Exact for every message size the spike produces (far below 2^53).
    #[expect(
        clippy::cast_precision_loss,
        reason = "message sizes are far below 2^53, where f64 is exact"
    )]
    let value = length as f64;
    value
}

/// Collects measurements and passes each one to the caller as soon as it is taken, so long runs show progress.
struct Recorder<'a> {
    suite: Suite,
    members: usize,
    report: &'a mut dyn FnMut(&Measurement),
    all: Vec<Measurement>,
}

impl Recorder<'_> {
    fn record(&mut self, metric: &str, value: f64, unit: &'static str) {
        let measurement = Measurement {
            suite: self.suite,
            members: self.members,
            metric: metric.to_owned(),
            value,
            unit,
        };
        (self.report)(&measurement);
        self.all.push(measurement);
    }

    fn size(&mut self, metric: &str, length: usize) {
        self.record(metric, bytes(length), "bytes");
    }

    fn millis(&mut self, metric: &str, values: Vec<f64>) {
        self.record(metric, median(values), "ms");
    }
}

/// Runs every measurement of `config`, reporting each as it is taken.
///
/// # Errors
///
/// Returns an error if any MLS operation fails.
pub fn run(
    config: &BenchConfig,
    report: &mut dyn FnMut(&Measurement),
) -> Result<Vec<Measurement>, BenchError> {
    let mut all = Vec::new();
    for &suite in &config.suites {
        for &members in &config.members {
            let mut recorder = Recorder {
                suite,
                members,
                report: &mut *report,
                all: Vec::new(),
            };
            measure_group(config, &mut recorder)?;
            if members <= config.healed_up_to {
                measure_healed(config, &mut recorder)?;
            }
            if Some(&members) == config.members.iter().max() {
                measure_past_epochs(&mut recorder)?;
            }
            all.append(&mut recorder.all);
        }
    }
    Ok(all)
}

/// One group of `recorder.members` members built with a single bulk add, then every operation once or `repetitions` times.
fn measure_group(config: &BenchConfig, recorder: &mut Recorder<'_>) -> Result<(), BenchError> {
    let suite = recorder.suite;
    let size = recorder.members;
    let mut validator = Validator::new(&[suite]);
    let mut directory = KeyPackageDirectory::new();
    let owner_id = DeviceId::parse("owner/device")?;
    let mut owner = Client::new(owner_id.clone())?;

    // Every other member: a real device with a real key package, published to (and checked by) the server. The first two keep their clients and really join.
    let mut members = Vec::with_capacity(size.saturating_sub(1));
    let mut active: BTreeMap<DeviceId, Client> = BTreeMap::new();
    let mut key_package_times = Vec::new();
    let mut key_package_size = 0;
    for index in 1..size {
        let device = DeviceId::parse(&format!("user{index}/device"))?;
        let client = Client::new(device.clone())?;
        let (key_package, millis) = timed(|| client.key_package(suite));
        let key_package = key_package?;
        key_package_times.push(millis);
        key_package_size = key_package.len();
        directory.publish(&device, &key_package)?;
        if active.len() < 2 {
            active.insert(device.clone(), client);
        }
        members.push(device);
    }
    let newcomer_id = DeviceId::parse("newcomer/device")?;
    let mut newcomer = Client::new(newcomer_id.clone())?;
    directory.publish(&newcomer_id, &newcomer.key_package(suite)?)?;
    if key_package_times.is_empty() {
        let (key_package, millis) = timed(|| newcomer.key_package(suite));
        key_package_size = key_package?.len();
        key_package_times.push(millis);
    }
    recorder.size("key_package", key_package_size);
    recorder.millis("key_package.create", key_package_times);

    // The group, registered with the server, then everyone else added in one commit.
    let group = owner.create_group(suite)?;
    validator.register_group(
        &owner_id,
        &owner.group_info(&group)?,
        &owner.ratchet_tree(&group)?,
    )?;
    let mut roster = owner.view(&group)?.roster;
    let mut change = Change::default();
    for device in &members {
        roster.set(device.user().clone(), Role::Editor);
        change.add.push(
            directory
                .fetch(device, suite)
                .ok_or_else(|| BenchError("no key package".to_owned()))?,
        );
    }
    change.roster = Some(roster);
    if !members.is_empty() {
        let (output, create) = timed(|| owner.commit(&group, &change));
        let output = output?;
        let (accepted, validate) = timed(|| validator.check_handshake(&owner_id, &output.commit));
        accepted?;
        owner.confirm_commit(&group)?;
        let welcome = output
            .welcome
            .ok_or_else(|| BenchError("no Welcome".to_owned()))?;
        recorder.size("bulk_add.commit", output.commit.len());
        recorder.size("bulk_add.welcome", welcome.len());
        recorder.millis("bulk_add.create", vec![create]);
        recorder.millis("bulk_add.validate", vec![validate]);
        let tree = validator.ratchet_tree(&group)?;
        recorder.size("ratchet_tree", tree.len());
        let mut join_times = Vec::new();
        for client in active.values_mut() {
            let (joined, millis) = timed(|| client.join(&welcome, &tree));
            joined?;
            join_times.push(millis);
        }
        recorder.millis("bulk_add.join", join_times);
    }

    // Self-updates (empty commits with an update path) by the active non-owner members, then by the owner: their first commits after the bulk add.
    let committers: Vec<DeviceId> = active
        .keys()
        .cloned()
        .chain(std::iter::once(owner_id.clone()))
        .collect();
    let mut sizes = Vec::new();
    let (mut creates, mut validates, mut processes) = (Vec::new(), Vec::new(), Vec::new());
    for repetition in 0..config.repetitions.max(1) {
        let committer = &committers[repetition % committers.len()];
        let output = {
            let client = if committer == &owner_id {
                &mut owner
            } else {
                active
                    .get_mut(committer)
                    .ok_or_else(|| BenchError("member".to_owned()))?
            };
            let (output, create) = timed(|| client.commit(&group, &Change::default()));
            creates.push(create);
            output?
        };
        let (accepted, validate) = timed(|| validator.check_handshake(committer, &output.commit));
        accepted?;
        validates.push(validate);
        sizes.push(bytes(output.commit.len()));
        let mut first_receiver = true;
        for (device, client) in std::iter::once((&owner_id, &mut owner)).chain(active.iter_mut()) {
            if device == committer {
                client.confirm_commit(&group)?;
            } else {
                let (received, process) = timed(|| client.receive(&group, &output.commit));
                received?;
                if first_receiver {
                    processes.push(process);
                    first_receiver = false;
                }
            }
        }
    }
    recorder.record("update.commit", median(sizes), "bytes");
    recorder.millis("update.create", creates);
    recorder.millis("update.validate", validates);
    if !processes.is_empty() {
        recorder.millis("update.process", processes);
    }

    // The owner shares the document with one more user: one commit adding the newcomer and their role.
    let mut roster = owner.view(&group)?.roster;
    roster.set(newcomer_id.user().clone(), Role::Viewer);
    let key_package = directory
        .fetch(&newcomer_id, suite)
        .ok_or_else(|| BenchError("no key package".to_owned()))?;
    let change = Change {
        add: vec![key_package],
        remove: Vec::new(),
        roster: Some(roster),
    };
    let (output, create) = timed(|| owner.commit(&group, &change));
    let output = output?;
    let (accepted, validate) = timed(|| validator.check_handshake(&owner_id, &output.commit));
    accepted?;
    owner.confirm_commit(&group)?;
    let mut processes = Vec::new();
    for client in active.values_mut() {
        let (received, process) = timed(|| client.receive(&group, &output.commit));
        received?;
        processes.push(process);
    }
    let welcome = output
        .welcome
        .ok_or_else(|| BenchError("no Welcome".to_owned()))?;
    let tree = validator.ratchet_tree(&group)?;
    let (joined, join) = timed(|| newcomer.join(&welcome, &tree));
    joined?;
    recorder.size("add_one.commit", output.commit.len());
    recorder.size("add_one.welcome", welcome.len());
    recorder.size("add_one.ratchet_tree", tree.len());
    recorder.millis("add_one.create", vec![create]);
    recorder.millis("add_one.validate", vec![validate]);
    if !processes.is_empty() {
        recorder.millis("add_one.process", processes);
    }
    recorder.millis("add_one.join", vec![join]);

    // The owner removes a member who never joined (the last one; with two members, the newcomer).
    let removed = members
        .iter()
        .rev()
        .find(|device| !active.contains_key(*device))
        .cloned()
        .unwrap_or_else(|| newcomer_id.clone());
    let mut roster = owner.view(&group)?.roster;
    roster.remove(removed.user());
    let change = Change {
        add: Vec::new(),
        remove: vec![removed.clone()],
        roster: Some(roster),
    };
    let (output, create) = timed(|| owner.commit(&group, &change));
    let output = output?;
    let (accepted, validate) = timed(|| validator.check_handshake(&owner_id, &output.commit));
    accepted?;
    owner.confirm_commit(&group)?;
    let mut processes = Vec::new();
    for client in active.values_mut().chain(std::iter::once(&mut newcomer)) {
        if client.device() == &removed {
            continue;
        }
        let (received, process) = timed(|| client.receive(&group, &output.commit));
        received?;
        processes.push(process);
    }
    recorder.size("remove_one.commit", output.commit.len());
    recorder.millis("remove_one.create", vec![create]);
    recorder.millis("remove_one.validate", vec![validate]);
    recorder.millis("remove_one.process", processes);

    // Updates of each payload size from the owner to a member who is still in the group.
    let receiver = if removed == newcomer_id {
        active
            .values_mut()
            .next()
            .ok_or_else(|| BenchError("no member left to receive".to_owned()))?
    } else {
        &mut newcomer
    };
    for &payload in &config.payloads {
        let repetitions: usize = if payload >= 100_000 { 10 } else { 50 };
        let (mut encrypts, mut validates, mut decrypts) = (Vec::new(), Vec::new(), Vec::new());
        let mut ciphertext = 0;
        for repetition in 0..repetitions {
            let body: Vec<u8> = (0..payload)
                .map(|index: usize| index.to_le_bytes()[0] ^ repetition.to_le_bytes()[0])
                .collect();
            let update = Update::new(UpdateKind::Content, body);
            let (message, encrypt) = timed(|| owner.send(&group, &update));
            let message = message?;
            let (accepted, validate) = timed(|| validator.check_application(&owner_id, &message));
            accepted?;
            let (received, decrypt) = timed(|| receiver.receive(&group, &message));
            received?;
            ciphertext = message.len();
            encrypts.push(encrypt);
            validates.push(validate);
            decrypts.push(decrypt);
        }
        recorder.size(&format!("message.{payload}.ciphertext"), ciphertext);
        recorder.millis(&format!("message.{payload}.encrypt"), encrypts);
        recorder.millis(&format!("message.{payload}.validate"), validates);
        recorder.millis(&format!("message.{payload}.decrypt"), decrypts);
    }

    // What the devices keep.
    recorder.size("state.owner", owner.state_size()?);
    recorder.size("state.member", receiver.state_size()?);
    recorder.size("state.server_tree", validator.ratchet_tree(&group)?.len());
    Ok(())
}

/// A group in which every member joined and committed once, so the tree has no blank inner nodes; then the commits a member makes from then on.
fn measure_healed(config: &BenchConfig, recorder: &mut Recorder<'_>) -> Result<(), BenchError> {
    let suite = recorder.suite;
    let size = recorder.members;
    if size < 3 {
        return Ok(());
    }
    let mut deployment = crate::sim::Deployment::new();
    let owner = deployment
        .add_device("owner/device", suite, 0)
        .map_err(|error| BenchError(error.to_string()))?;
    let group = deployment
        .create_group(&owner, suite)
        .map_err(|error| BenchError(error.to_string()))?;
    let mut devices = Vec::new();
    for index in 1..size {
        let device = deployment
            .add_device(&format!("user{index}/device"), suite, 1)
            .map_err(|error| BenchError(error.to_string()))?;
        devices.push((device, Role::Editor));
    }
    deployment
        .add_members(&owner, &group, &devices)
        .map_err(|error| BenchError(error.to_string()))?;
    for (device, _) in &devices {
        deployment
            .self_update(device, &group)
            .map_err(|error| BenchError(error.to_string()))?;
    }
    let (mut sizes, mut creates, mut processes) = (Vec::new(), Vec::new(), Vec::new());
    for repetition in 0..config.repetitions.max(1) {
        let committer = &devices[(repetition * 7 + 3) % devices.len()].0;
        let (output, create) = timed(|| {
            deployment
                .client_mut(committer)
                .commit(&group, &Change::default())
        });
        let output = output?;
        deployment
            .validator
            .check_handshake(committer, &output.commit)?;
        deployment.client_mut(committer).confirm_commit(&group)?;
        let (received, process) = timed(|| {
            deployment
                .client_mut(&owner)
                .receive(&group, &output.commit)
        });
        received?;
        for (device, _) in &devices {
            if device != committer {
                deployment
                    .client_mut(device)
                    .receive(&group, &output.commit)?;
            }
        }
        sizes.push(bytes(output.commit.len()));
        creates.push(create);
        processes.push(process);
    }
    recorder.record("healed.update.commit", median(sizes), "bytes");
    recorder.millis("healed.update.create", creates);
    recorder.millis("healed.update.process", processes);
    Ok(())
}

/// The cost of keeping past epochs' keys (see [`crate::client::DEFAULT_PAST_EPOCHS`]): OpenMLS stores every member's leaf for each past epoch and rewrites that record after every message, so in large groups each kept epoch makes every update slower to send and the device's state larger.
fn measure_past_epochs(recorder: &mut Recorder<'_>) -> Result<(), BenchError> {
    let suite = recorder.suite;
    for past_epochs in 0..=2 {
        let mut owner = Client::with_past_epochs(DeviceId::parse("owner/device")?, past_epochs)?;
        let group = owner.create_group(suite)?;
        let mut change = Change::default();
        let mut roster = owner.view(&group)?.roster;
        for index in 1..recorder.members {
            let device = DeviceId::parse(&format!("user{index}/device"))?;
            change
                .add
                .push(Client::new(device.clone())?.key_package(suite)?);
            roster.set(device.user().clone(), Role::Editor);
        }
        change.roster = Some(roster);
        // The bulk add, then enough self-updates that every kept past epoch is filled.
        owner.commit(&group, &change)?;
        owner.confirm_commit(&group)?;
        for _ in 0..past_epochs {
            owner.commit(&group, &Change::default())?;
            owner.confirm_commit(&group)?;
        }
        let update = Update::new(UpdateKind::Content, vec![0x42; 100]);
        let mut encrypts = Vec::new();
        for _ in 0..20 {
            let (message, millis) = timed(|| owner.send(&group, &update));
            message?;
            encrypts.push(millis);
        }
        recorder.millis(
            &format!("past_epochs.{past_epochs}.message.100.encrypt"),
            encrypts,
        );
        recorder.size(
            &format!("past_epochs.{past_epochs}.state.owner"),
            owner.state_size()?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test(unsupported = test)]
    fn the_smoke_run_measures_everything() {
        let mut seen = 0;
        let measurements = run(&BenchConfig::smoke(), &mut |_| seen += 1).unwrap();
        assert_eq!(seen, measurements.len());
        for metric in [
            "key_package",
            "bulk_add.welcome",
            "update.commit",
            "add_one.join",
            "remove_one.process",
            "message.100.decrypt",
            "state.member",
        ] {
            assert!(
                measurements
                    .iter()
                    .any(|measurement| measurement.metric == metric && measurement.members == 10),
                "{metric} missing"
            );
        }
        assert!(
            measurements
                .iter()
                .any(|measurement| measurement.metric == "healed.update.commit")
        );
        assert!(
            measurements
                .iter()
                .any(|measurement| measurement.metric == "past_epochs.2.state.owner")
        );
        assert!(
            measurements
                .iter()
                .all(|measurement| measurement.value.is_finite() && measurement.value >= 0.0)
        );
        assert_eq!(median(vec![3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), 2.5);
    }
}
