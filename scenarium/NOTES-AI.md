# scenarium - Implementation Notes (AI)

Core data structures for node-based graph execution with async lambdas.

## Key Modules

| Module | Description |
|--------|-------------|
| `graph.rs` | Core `Graph` struct with `NodeId`, `Node`, `Input`, `Binding`, `Event` |
| `function.rs` | Function library definitions (`FuncLib`, `Func`, `FuncInput`, `FuncOutput`) |
| `data.rs` | Data type and value systems (`DataType`, `StaticValue`, `DynamicValue`) |
| `execution_graph.rs` | Execution scheduling and state management |
| `func_lambda.rs` | Async function lambdas (`FuncLambda`, `AsyncLambdaFn`) |
| `event_lambda.rs` | Async event callbacks (`EventLambda`) |
| `context.rs` | Context registry for invocation environments (`ContextManager`) |
| `worker.rs` | Background tokio thread for graph execution |
| `compute.rs` | Node invocation and value conversion |

## Built-in Functions (elements/)

| Module | Description |
|--------|-------------|
| `elements/basic_funclib.rs` | Math operations (add, subtract, multiply, divide, power, log, etc.) |
| `elements/worker_events_funclib.rs` | Timer/frame events with FPS tracking |
| `elements/lua/` | Lua function loading and invocation via mlua |

## Key Data Structures

### Graph Elements

```rust
NodeId(uuid)      // Unique node identifier
FuncId(uuid)      // Unique function identifier  
TypeId(uuid)      // Custom type identifier
PortAddress       // { target_id: NodeId, port_idx: usize }
EventRef          // { node_id: NodeId, event_idx: usize }
```

### Binding System

```rust
Binding:
  - None                    // Unconnected
  - Const(StaticValue)      // Constant value
  - Bind(PortAddress)       // Data flow from another node's output
```

### Execution State

```rust
InputState: Changed | Unchanged
OutputUsage: Skip | Needed
ExecutionBehavior: Impure | Pure | Once
NodeBehavior: AsFunction | Once
```

### Value Types

```rust
DataType: Null | Float | Int | Bool | String | Array | Custom(TypeId)
StaticValue: Serializable constants (f64 equality via to_bits())
DynamicValue: Runtime values including Arc<dyn Any + Send + Sync> for Custom variant (shallow clone)
```

## Architecture Patterns

### Lambda-Based Function Execution
- Functions defined as async Rust closures (`FuncLambda`)
- Lua scripting integration via mlua
- Async/await support throughout execution
- `async_lambda!` macro reduces boilerplate

### Execution Graph Scheduling
- DFS-based topological ordering with cycle detection
- Three-phase scheduling: backward (collect order), forward (propagate state), backward (collect execute order)
- Input state tracking for pure function optimization
- Output usage counting to skip unnecessary computations
- See `src/execution_graph_NOTES-AI.md` for detailed research and industry comparison

### Event System
- Nodes emit named events with subscriber lists
- Event lambdas run in async worker loop
- Frame events with configurable frequency (`FpsEventState`)

### Worker Pattern
- Background tokio task receives messages (Update, Event, Clear, ExecuteTerminals)
- Execution callbacks with stats (elapsed time, node timings, missing inputs)
- Event loop with stop flag and notify broadcast
- `EventLoopHandle` for controlling event loops

### State Management
- `AnyState` - Type-erased HashMap for per-node state
- `SharedAnyState` - Arc<Mutex<AnyState>> for event state sharing
- `ContextManager` - Lazy-initialized context registry
- `Slot` - Wait-friendly value passing between async boundaries

## Testing

- Unit tests in-module via `#[cfg(test)]` blocks
- `test_graph()` and `test_func_lib()` fixtures for reproducible testing
- Criterion benchmarks in `scenarium/benches/b1.rs`
- `TestFuncHooks` with Arc callbacks for async test support

## Dependencies

common, tokio, serde, uuid, anyhow, thiserror, strum, mlua (Lua54), glam, hashbrown, criterion
