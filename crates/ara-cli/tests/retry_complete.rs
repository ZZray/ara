//! Source consumers for fixed OMP596f2da complete AI-RETRY41 and six contracts.
//! One existing NativePromiseRun drives graph jobs; the fixture has no pump.
use ara_ai::{ActualStreamFailureReceipt, graph_values::GraphValueFactory, retry::RetryBindings};
use ara_cli::{
    mock_provider_host::NativeCallingRun,
    native_realm::{
        NativeRealm,
        promise::{NativeDrain, NativePromiseRun},
    },
    retry_host::{NativeRetryBindings, signal::NativeSignalDomain},
};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot};
use std::{future::Future, sync::Arc, time::Duration};
pub type Result<T> = std::result::Result<T, ValueError>;
#[path = "retry_complete/owner_a.rs"]
mod owner_a;
#[path = "retry_complete/owner_b.rs"]
mod owner_b;
#[path = "retry_complete/root_date.rs"]
mod root_date;

pub struct Harness {
    pub realm: Arc<NativeRealm>,
    pub driver: NativePromiseRun,
    pub factory: Arc<GraphValueFactory>,
    pub bindings: Arc<dyn RetryBindings>,
    pub signals: Arc<NativeSignalDomain>,
    pub frozen: NativeCallingRun,
}
impl Harness {
    pub fn new() -> Result<Self> {
        let realm = NativeRealm::prepare()?;
        let driver = realm.begin_promise_run()?;
        let factory = realm.values.factory().clone();
        let admission = driver.admission();
        let frozen = {
            let runtime = admission.scoped(driver.root_scope(), driver.root_parent())?;
            NativeCallingRun::capture(&factory, &realm.values, admission.clone(), &runtime)?
        };
        let native = NativeRetryBindings::prepare(
            &factory,
            &realm,
            frozen.clone(),
            ActualStreamFailureReceipt::default(),
        )?;
        let signals = native.signal_domain();
        Ok(Self {
            realm,
            driver,
            factory,
            bindings: native,
            signals,
            frozen,
        })
    }
    pub fn heap(&self) -> &Arc<ValueHeap> {
        self.factory.heap()
    }
    pub fn with<T>(&self, operation: impl FnOnce(&dyn ValueRuntime) -> Result<T>) -> Result<T> {
        let mut operation = Some(operation);
        let mut result = None;
        self.frozen.with_runtime(&self.factory, &mut |runtime| {
            result = Some(operation.take().ok_or(
                ValueError::UnsupportedOperation("Fixture runtime called twice"),
            )?(runtime)?);
            self.heap().undefined()
        })?;
        result.ok_or(ValueError::UnsupportedOperation(
            "Fixture runtime omitted operation",
        ))
    }
    pub fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.with(|runtime| self.heap().get(value, &key.into(), runtime))
    }
    pub fn put(&self, value: &ValueHandle, key: &str, item: ValueHandle) -> Result<()> {
        self.with(|runtime| {
            self.heap()
                .define_own_data(value, key.into(), item, runtime)
        })
    }
    pub fn s(&self, value: &ValueHandle) -> Result<String> {
        match self.heap().snapshot(value)? {
            ValueSnapshot::String(value) => value
                .to_utf8()
                .map_err(|_| ValueError::UnsupportedOperation("Fixture string not scalar")),
            _ => Err(ValueError::UnsupportedOperation(
                "Fixture expected original string",
            )),
        }
    }
    pub fn n(&self, value: &ValueHandle) -> Result<f64> {
        self.with(|runtime| self.heap().to_number(value, runtime))
    }
    pub fn data(&self, value: serde_json::Value) -> Result<ValueHandle> {
        self.heap().import_serde(&value)
    }
    pub fn object(&self, entries: &[(&str, ValueHandle)]) -> Result<ValueHandle> {
        self.with(|runtime| {
            let object = self.heap().object()?;
            for (key, value) in entries {
                self.heap()
                    .define_own_data(&object, (*key).into(), value.clone(), runtime)?;
            }
            Ok(object)
        })
    }
    pub async fn run_until<T>(&mut self, future: impl Future<Output = Result<T>>) -> Result<T> {
        tokio::pin!(future);
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let wake = self.driver.wake();
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let mut burst = 0;
                while matches!(self.driver.drain_one()?, NativeDrain::RanJob { .. }) {
                    burst += 1;
                    if burst == 64 {
                        tokio::task::yield_now().await;
                        break;
                    }
                }
                tokio::select! {result=&mut future=>return result,_=&mut notified=>{}}
            }
        })
        .await
        .map_err(|_| {
            ValueError::UnsupportedOperation(
                "Original Retry consumer did not settle in its existing driver bound",
            )
        })?
    }
}

#[tokio::test]
async fn original_41_and_six_source_contracts() {
    if tokio::time::timeout(Duration::from_secs(15), owner_b::provider_cap_child())
        .await
        .expect("Real provider-cap child route exceeded its bound")
        .expect("Real provider-cap child route failed")
    {
        return;
    }
    let mut h = Harness::new().expect("same-Session native Retry imports");
    owner_a::original33(&mut h)
        .await
        .expect("original stream13/oneshot20 consumers");
    owner_b::original8(&mut h)
        .await
        .expect("original localwork4/pre-response4 consumers");
    owner_a::source_groups(&mut h)
        .await
        .expect("stream and oneshot source contracts");
    owner_b::source_groups(&mut h)
        .await
        .expect("remaining four source contracts");
    eprintln!("ARA-RETRY41 COMPLETE 41/41");
}
