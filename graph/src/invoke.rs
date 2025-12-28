use std::any::Any;
use std::fmt::{Debug, Formatter};

use async_trait::async_trait;

use hashbrown::HashMap;

use crate::data::DynamicValue;
use crate::function::{Func, FuncId, FuncLib};
use crate::graph::NodeBehavior;

pub type InvokeArgs = [DynamicValue];

pub type Lambda =
    dyn Fn(&mut InvokeCache, &mut InvokeArgs, &mut InvokeArgs) + Send + Sync + 'static;

#[derive(Debug, Default)]
pub struct InvokeCache {
    boxed: Option<Box<dyn Any + Send>>,
}

#[async_trait]
pub trait Invoker: Debug + Send + Sync {
    fn get_func_lib(&self) -> FuncLib;
    async fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct UberInvoker {
    invokers: Vec<Box<dyn Invoker>>,
    func_lib: FuncLib,
    function_id_to_invoker_index: HashMap<FuncId, usize>,
}

#[derive(Default)]
pub struct LambdaInvoker {
    func_lib: FuncLib,
    lambdas: HashMap<FuncId, Box<Lambda>>,
}

impl InvokeCache {
    pub(crate) fn default() -> InvokeCache {
        InvokeCache { boxed: None }
    }

    pub fn is_none(&self) -> bool {
        self.boxed.is_none()
    }

    pub fn is_some<T>(&self) -> bool
    where
        T: Any + Send,
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
    where
        T: Any + Send,
    {
        self.boxed = Some(Box::new(value));
    }

    pub fn get_or_default<T>(&mut self) -> &mut T
    where
        T: Any + Send + Default,
    {
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed
                .as_mut()
                .expect("InvokeCache missing value")
                .downcast_mut::<T>()
                .expect("InvokeCache has unexpected type")
        } else {
            self.boxed
                .insert(Box::<T>::default())
                .downcast_mut::<T>()
                .expect("InvokeCache default insert failed")
        }
    }
    pub fn get_or_default_with<T, F>(&mut self, f: F) -> &mut T
    where
        T: Any + Send,
        F: FnOnce() -> T,
    {
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed
                .as_mut()
                .expect("InvokeCache missing value")
                .downcast_mut::<T>()
                .expect("InvokeCache has unexpected type")
        } else {
            self.boxed
                .insert(Box::<T>::new(f()))
                .downcast_mut::<T>()
                .expect("InvokeCache insert failed")
        }
    }
}

impl LambdaInvoker {
    pub fn add_lambda<F>(&mut self, function: Func, lambda: F)
    where
        F: Fn(&mut InvokeCache, &mut InvokeArgs, &mut InvokeArgs) + Send + Sync + 'static,
    {
        if self.lambdas.contains_key(&function.id) {
            panic!(
                "Function {}:{} with the same id already exists.",
                function.id, function.name
            );
        }

        self.lambdas.insert(function.id, Box::new(lambda));

        self.func_lib.add(function);
    }
}

impl UberInvoker {
    pub fn with<It>(invokers: It) -> Self
    where
        It: IntoIterator<Item = Box<dyn Invoker>>,
    {
        let mut invokers: Vec<Box<dyn Invoker>> = invokers.into_iter().collect();
        let mut function_id_to_invoker_index = HashMap::new();
        let mut func_lib = FuncLib::default();

        invokers.iter_mut().enumerate().for_each(|(idx, invoker)| {
            let new_func_lib = invoker.get_func_lib();
            new_func_lib.funcs.iter().for_each(|func| {
                function_id_to_invoker_index.insert(func.id, idx);
            });

            func_lib.merge(new_func_lib);
        });

        Self {
            invokers,
            func_lib,
            function_id_to_invoker_index,
        }
    }

    pub fn merge<T>(&mut self, mut invoker: T)
    where
        T: Invoker + Any + 'static,
    {
        if let Some(other_uber) = (&mut invoker as &mut dyn Any).downcast_mut::<UberInvoker>() {
            self.func_lib
                .merge(std::mem::take(&mut other_uber.func_lib));

            let idx = self.invokers.len();
            self.invokers.append(&mut other_uber.invokers);
            other_uber
                .function_id_to_invoker_index
                .iter()
                .for_each(|(&func_id, &old_idx)| {
                    self.function_id_to_invoker_index
                        .insert(func_id, idx + old_idx);
                });
        } else {
            let idx = self.invokers.len();
            let other_func_lib = invoker.get_func_lib();
            other_func_lib.funcs.iter().for_each(|func| {
                self.function_id_to_invoker_index.insert(func.id, idx);
            });

            self.func_lib.merge(other_func_lib);
            self.invokers.push(Box::new(invoker));
        }
    }
}

#[async_trait]
impl Invoker for UberInvoker {
    fn get_func_lib(&self) -> FuncLib {
        self.func_lib.clone()
    }
    async fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        assert!(!self.invokers.is_empty(), "No invokers available");

        let invoker = if self.invokers.len() == 1 {
            self.invokers
                .first()
                .expect("Invoker list unexpectedly empty")
        } else {
            let &invoker_index = self
                .function_id_to_invoker_index
                .get(&function_id)
                .expect("Missing invoker for function_id");

            &self.invokers[invoker_index]
        };

        invoker.invoke(function_id, cache, inputs, outputs).await
    }
}

#[async_trait]
impl Invoker for LambdaInvoker {
    fn get_func_lib(&self) -> FuncLib {
        self.func_lib.clone()
    }
    async fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        let invokable = self
            .lambdas
            .get(&function_id)
            .expect("Missing lambda for function_id");
        (invokable)(cache, inputs, outputs);

        Ok(())
    }
}

impl Debug for LambdaInvoker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LambdaInvoker")
            .field("func_lib", &self.func_lib)
            .field("lambdas", &self.lambdas.len())
            .finish()
    }
}

#[cfg(test)]
pub(crate) fn create_invoker<GetA, GetB, SetResult>(
    get_a: GetA,
    get_b: GetB,
    result: SetResult,
) -> LambdaInvoker
where
    SetResult: Fn(i64) + Send + Sync + 'static,
    GetA: Fn() -> i64 + Send + Sync + 'static,
    GetB: Fn() -> i64 + Send + Sync + 'static,
{
    use crate::function::test_func_lib;

    let func_lib = test_func_lib();

    let mut invoker = LambdaInvoker::default();

    // print
    invoker.add_lambda(
        func_lib
            .by_name("print")
            .unwrap_or_else(|| panic!("Func named \"print\" not found"))
            .clone(),
        move |_, inputs, _| {
            assert_eq!(
                inputs.len(),
                1,
                "print expects exactly 1 input but received {}",
                inputs.len()
            );
            result(inputs[0].as_int());
        },
    );
    // val 1
    invoker.add_lambda(
        func_lib
            .by_name("get_a")
            .unwrap_or_else(|| panic!("Func named \"get_a\" not found"))
            .clone(),
        move |_, _, outputs| {
            assert_eq!(
                outputs.len(),
                1,
                "get_a expects exactly 1 output but received {}",
                outputs.len()
            );
            outputs[0] = (get_a() as f64).into();
        },
    );
    // val 2
    invoker.add_lambda(
        func_lib
            .by_name("get_b")
            .unwrap_or_else(|| panic!("Func named \"get_b\" not found"))
            .clone(),
        move |_, _, outputs| {
            assert_eq!(
                outputs.len(),
                1,
                "get_b expects exactly 1 output but received {}",
                outputs.len()
            );
            outputs[0] = (get_b() as f64).into();
        },
    );
    // sum
    invoker.add_lambda(
        func_lib
            .by_name("sum")
            .unwrap_or_else(|| panic!("Func named \"sum\" not found"))
            .clone(),
        |ctx, inputs, outputs| {
            assert_eq!(
                inputs.len(),
                2,
                "sum expects exactly 2 inputs but received {}",
                inputs.len()
            );
            assert_eq!(
                outputs.len(),
                1,
                "sum expects exactly 1 output but received {}",
                outputs.len()
            );
            let a: i64 = inputs[0].as_int();
            let b: i64 = inputs[1].as_int();
            ctx.set(a + b);
            outputs[0] = (a + b).into();
        },
    );
    // mult
    invoker.add_lambda(
        func_lib
            .by_name("mult")
            .unwrap_or_else(|| panic!("Func named \"mult\" not found"))
            .clone(),
        |ctx, inputs, outputs| {
            assert_eq!(
                inputs.len(),
                2,
                "mult expects exactly 2 inputs but received {}",
                inputs.len()
            );
            assert_eq!(
                outputs.len(),
                1,
                "mult expects exactly 1 output but received {}",
                outputs.len()
            );
            let a: i64 = inputs[0].as_int();
            let b: i64 = inputs[1].as_int();
            outputs[0] = (a * b).into();
            ctx.set(a * b);
        },
    );

    invoker
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use crate::{
        data::DynamicValue,
        elements::{basic_invoker::BasicInvoker, timers_invoker::TimersInvoker},
    };

    use super::*;

    #[test]
    fn invoke_context_test() -> anyhow::Result<()> {
        fn box_test_(cache: &mut InvokeCache) {
            let n = *cache.get_or_default::<u32>();
            assert_eq!(n, 0);
            let n = cache.get_or_default::<i32>();
            assert_eq!(*n, 0);
            *n = 13;
        }

        let mut cache = InvokeCache::default();
        box_test_(&mut cache);
        let n = cache.get_or_default::<i32>();
        assert_eq!(*n, 13);
        let n = *cache.get_or_default::<u32>();
        assert_eq!(n, 0);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_invoker_sum_lambda() -> anyhow::Result<()> {
        let invoker = create_invoker(|| 2, || 3, |_| {});
        let sum_id = invoker
            .get_func_lib()
            .by_name("sum")
            .expect("Func named \"sum\" not found")
            .id;
        let mut cache = InvokeCache::default();
        let mut inputs = vec![DynamicValue::Int(2), DynamicValue::Int(3)];
        let mut outputs = vec![DynamicValue::None];

        invoker
            .invoke(sum_id, &mut cache, &mut inputs, &mut outputs)
            .await?;

        assert_eq!(outputs[0].as_int(), 5);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_invoker_print_lambda() -> anyhow::Result<()> {
        let printed = Arc::new(Mutex::new(-1i64));
        let printed_ref = printed.clone();
        let invoker = create_invoker(
            || 0,
            || 0,
            move |value| {
                *printed_ref
                    .try_lock()
                    .expect("Printed value mutex is already locked") = value;
            },
        );
        let print_id = invoker
            .get_func_lib()
            .by_name("print")
            .expect("Func named \"print\" not found")
            .id;
        let mut cache = InvokeCache::default();
        let mut inputs = vec![DynamicValue::Int(9)];
        let mut outputs = vec![];

        invoker
            .invoke(print_id, &mut cache, &mut inputs, &mut outputs)
            .await?;

        assert_eq!(*printed.lock().await, 9);

        Ok(())
    }

    #[test]
    fn user_invoker_merge() -> anyhow::Result<()> {
        let uber1 = UberInvoker::with(vec![Box::<BasicInvoker>::default() as Box<dyn Invoker>]);
        let uber2 = UberInvoker::with(vec![Box::<TimersInvoker>::default() as Box<dyn Invoker>]);

        let mut uber = UberInvoker::default();
        uber.merge(uber1);
        uber.merge(uber2);

        assert_eq!(uber.invokers.len(), 2);
        assert_eq!(uber.function_id_to_invoker_index.len(), 18);

        let basic_invoker_func_count = uber
            .function_id_to_invoker_index
            .values()
            .filter(|&&idx| idx == 0)
            .count();
        assert_eq!(basic_invoker_func_count, 17);

        let timers_invoker_func_count = uber
            .function_id_to_invoker_index
            .values()
            .filter(|&&idx| idx == 1)
            .count();
        assert_eq!(timers_invoker_func_count, 1);

        Ok(())
    }
}
