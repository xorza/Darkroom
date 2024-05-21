use std::any::Any;
use std::fmt::{Debug, Formatter};

use hashbrown::HashMap;

use crate::data::DynamicValue;
use crate::function::{Func, FuncId, FuncLib};

pub type InvokeArgs = [DynamicValue];

pub type Lambda = dyn Fn(&mut InvokeCache, &InvokeArgs, &mut InvokeArgs) + Send + Sync + 'static;

#[derive(Debug, Default)]
pub struct InvokeCache {
    boxed: Option<Box<dyn Any>>,
}

pub trait Invoker: Debug + Send + Sync {
    fn get_func_lib(&self) -> &FuncLib;
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
    pub(crate) func_lib: FuncLib,
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
            T: Any,
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
        where
            T: Any,
    {
        self.boxed
            .as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
        where
            T: Any,
    {
        self.boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
        where
            T: Any,
    {
        self.boxed = Some(Box::new(value));
    }

    pub fn get_or_default<T>(&mut self) -> &mut T
        where
            T: Any + Default,
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
            T: Any,
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
            F: Fn(&mut InvokeCache, &InvokeArgs, &mut InvokeArgs) + Send + Sync + 'static,
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
            new_func_lib.iter().for_each(|(id, _func)| {
                function_id_to_invoker_index.insert(id.clone(), idx);
            });

            func_lib.merge(new_func_lib);
        });

        Self {
            invokers,
            func_lib,
            function_id_to_invoker_index,
        }
    }
}

impl<It> From<It> for UberInvoker
    where
        It: IntoIterator<Item=Box<dyn Invoker>>,
{
    fn from(invokers: It) -> Self {
        Self::new(invokers.into_iter().collect())
    }
}

impl Invoker for UberInvoker {
    fn get_func_lib(&self) -> &FuncLib {
        &self.func_lib
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
    fn get_func_lib(&self) -> &FuncLib {
        &self.func_lib
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
