use std::any::Any;
use std::fmt::{Debug, Formatter};

use hashbrown::HashMap;

use crate::data::DynamicValue;
use crate::function::{Func, FuncId, FuncLib};

pub type InvokeArgs = [DynamicValue];

pub type Lambda =
    dyn Fn(&mut InvokeCache, &mut InvokeArgs, &mut InvokeArgs) + Send + Sync + 'static;

#[derive(Debug, Default)]
pub struct InvokeCache {
    boxed: Option<Box<dyn Any + Send>>,
}

pub trait Invoker: Debug + Send + Sync {
    fn get_func_lib(&self) -> FuncLib;
    fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;
}

#[derive(Default)]
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
            self.boxed.as_mut().unwrap().downcast_mut::<T>().unwrap()
        } else {
            self.boxed
                .insert(Box::<T>::default())
                .downcast_mut::<T>()
                .unwrap()
        }
    }
    pub fn get_or_default_with<T, F>(&mut self, f: F) -> &mut T
    where
        T: Any + Send,
        F: FnOnce() -> T,
    {
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed.as_mut().unwrap().downcast_mut::<T>().unwrap()
        } else {
            self.boxed
                .insert(Box::<T>::new(f()))
                .downcast_mut::<T>()
                .unwrap()
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
    pub fn new(mut invokers: Vec<Box<dyn Invoker>>) -> Self {
        let mut function_id_to_invoker_index = HashMap::new();
        let mut func_lib = FuncLib::default();

        invokers.iter_mut().enumerate().for_each(|(idx, invoker)| {
            let new_func_lib = invoker.get_func_lib();
            new_func_lib.iter().for_each(|func| {
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
            other_func_lib.iter().for_each(|func| {
                self.function_id_to_invoker_index.insert(func.id, idx);
            });

            self.func_lib.merge(other_func_lib);
            self.invokers.push(Box::new(invoker));
        }
    }
}

impl<It> From<It> for UberInvoker
where
    It: IntoIterator<Item = Box<dyn Invoker>>,
{
    fn from(invokers: It) -> Self {
        Self::new(invokers.into_iter().collect())
    }
}

impl Invoker for UberInvoker {
    fn get_func_lib(&self) -> FuncLib {
        self.func_lib.clone()
    }
    fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        assert!(!self.invokers.is_empty(), "No invokers available");

        let invoker = if self.invokers.len() == 1 {
            self.invokers.first().unwrap()
        } else {
            let &invoker_index = self
                .function_id_to_invoker_index
                .get(&function_id)
                .expect("Missing invoker for function_id");

            &self.invokers[invoker_index]
        };

        invoker.invoke(function_id, cache, inputs, outputs)
    }
}

impl Invoker for LambdaInvoker {
    fn get_func_lib(&self) -> FuncLib {
        self.func_lib.clone()
    }
    fn invoke(
        &self,
        function_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        let invokable = self.lambdas.get(&function_id).unwrap();
        (invokable)(cache, inputs, outputs);

        Ok(())
    }
}

impl Debug for UberInvoker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UberInvoker")
            .field("invokers", &self.invokers)
            .field(
                "function_id_to_invoker_index",
                &self.function_id_to_invoker_index,
            )
            .field("func_lib", &self.func_lib)
            .finish()
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
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use crate::compute::Compute;
    use crate::data::StaticValue;
    use crate::elements::basic_invoker::BasicInvoker;
    use crate::elements::timers_invoker::TimersInvoker;
    use crate::function::{FuncBehavior, FuncLib};
    use crate::graph::{Binding, Graph};
    use crate::invoke::{InvokeCache, Invoker, LambdaInvoker, UberInvoker};
    use crate::runtime_graph::RuntimeGraph;

    struct TestValues {
        a: i64,
        b: i64,
        result: i64,
    }

    fn create_invoker<GetA, GetB, SetResult>(
        get_a: GetA,
        get_b: GetB,
        result: SetResult,
    ) -> anyhow::Result<LambdaInvoker>
    where
        SetResult: Fn(i64) + Send + Sync + 'static,
        GetA: Fn() -> i64 + Send + Sync + 'static,
        GetB: Fn() -> i64 + Send + Sync + 'static,
    {
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let mut invoker = LambdaInvoker::default();

        // print
        invoker.add_lambda(
            func_lib.func_by_name("print").unwrap().clone(),
            move |_, inputs, _| {
                result(inputs[0].as_int());
            },
        );
        // val 1
        invoker.add_lambda(
            func_lib.func_by_name("get_a").unwrap().clone(),
            move |_, _, outputs| {
                outputs[0] = (get_a() as f64).into();
            },
        );
        // val 2
        invoker.add_lambda(
            func_lib.func_by_name("get_b").unwrap().clone(),
            move |_, _, outputs| {
                outputs[0] = (get_b() as f64).into();
            },
        );
        // sum
        invoker.add_lambda(
            func_lib.func_by_name("sum").unwrap().clone(),
            |ctx, inputs, outputs| {
                let a: i64 = inputs[0].as_int();
                let b: i64 = inputs[1].as_int();
                ctx.set(a + b);
                outputs[0] = (a + b).into();
            },
        );
        // mult
        invoker.add_lambda(
            func_lib.func_by_name("mult").unwrap().clone(),
            |ctx, inputs, outputs| {
                let a: i64 = inputs[0].as_int();
                let b: i64 = inputs[1].as_int();
                outputs[0] = (a * b).into();
                ctx.set(a * b);
            },
        );

        Ok(invoker)
    }

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

    #[test]
    fn simple_compute_test() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut invoker = create_invoker(
            move || test_values_a.lock().a,
            move || test_values_b.lock().b,
            move |result| test_values_result.lock().result = result,
        )?;

        let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

        let mut runtime_graph = RuntimeGraph::new(&graph, &invoker.func_lib);
        Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;
        assert_eq!(test_values.lock().result, 35);

        Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;
        assert_eq!(test_values.lock().result, 35);

        test_values.lock().b = 7;
        invoker.func_lib.func_by_name_mut("get_b").unwrap().behavior = FuncBehavior::Active;
        let mut runtime_graph = RuntimeGraph::new(&graph, &invoker.func_lib);
        Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;
        assert_eq!(test_values.lock().result, 63);

        Ok(())
    }

    #[test]
    fn default_input_value() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));
        let test_values_result = test_values.clone();

        let invoker = create_invoker(
            || panic!("Unexpected call to get_a"),
            || panic!("Unexpected call to get_b"),
            move |result| test_values_result.lock().result = result,
        )?;
        let func_lib = invoker.get_func_lib();
        let compute = Compute::default();

        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

        {
            let sum_inputs = &mut graph.node_by_name_mut("sum").unwrap().inputs;
            sum_inputs[0].const_value = Some(StaticValue::from(29));
            sum_inputs[0].binding = Binding::Const;
            sum_inputs[1].const_value = Some(StaticValue::from(11));
            sum_inputs[1].binding = Binding::Const;
        }

        {
            let mult_inputs = &mut graph.node_by_name_mut("mult").unwrap().inputs;
            mult_inputs[1].const_value = Some(StaticValue::from(9));
            mult_inputs[1].binding = Binding::Const;
        }

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);

        compute.run(&graph, &func_lib, &invoker, &mut runtime_graph)?;
        assert_eq!(test_values.lock().result, 360);

        drop(graph);

        Ok(())
    }

    #[test]
    fn cached_value() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut invoker = create_invoker(
            move || {
                let a1 = test_values_a.lock().a;
                test_values_a.lock().a += 1;

                a1
            },
            move || {
                let b1 = test_values_b.lock().b;
                test_values_b.lock().b += 1;
                if b1 == 6 {
                    panic!("Unexpected call to get_b");
                }

                b1
            },
            move |result| test_values_result.lock().result = result,
        )?;

        invoker.func_lib.func_by_name_mut("get_a").unwrap().behavior = FuncBehavior::Active;

        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        graph.node_by_name_mut("sum").unwrap().cache_outputs = false;

        let mut runtime_graph = RuntimeGraph::new(&graph, &invoker.func_lib);
        Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;

        //assert that both nodes were called
        assert_eq!(test_values.lock().a, 3);
        assert_eq!(test_values.lock().b, 6);
        assert_eq!(test_values.lock().result, 35);

        Compute::default().run(&graph, &invoker.func_lib, &invoker, &mut runtime_graph)?;

        //assert that node a was called again
        assert_eq!(test_values.lock().a, 4);
        //but node b was cached
        assert_eq!(test_values.lock().b, 6);
        assert_eq!(test_values.lock().result, 40);

        drop(graph);

        Ok(())
    }

    #[test]
    fn user_invoker_merge() -> anyhow::Result<()> {
        let uber1 = UberInvoker::new(vec![Box::<BasicInvoker>::default()]);
        let uber2 = UberInvoker::new(vec![Box::<TimersInvoker>::default()]);

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
