use std::mem;
use scrypto::core::{FnIdent, MethodIdent, ReceiverMethodIdent};
use transaction::errors::IdAllocationError;
use transaction::model::{AuthZoneParams, Instruction};
use transaction::validation::*;

use crate::engine::call_frame::SubstateLock;
use crate::engine::*;
use crate::fee::FeeReserve;
use crate::model::*;
use crate::types::*;
use crate::wasm::*;

#[macro_export]
macro_rules! trace {
    ( $self: expr, $level: expr, $msg: expr $( , $arg:expr )* ) => {
        #[cfg(not(feature = "alloc"))]
        if $self.trace {
            println!("{}[{:5}] {}", "  ".repeat($self.current_frame.depth) , $level, sbor::rust::format!($msg, $( $arg ),*));
        }
    };
}

pub enum ScryptoFnIdent {
    Function(PackageAddress, String, String),
    Method(ResolvedReceiver, String),
}

pub struct Kernel<
    'g, // Lifetime of values outliving all frames
    's, // Substate store lifetime
    W,  // WASM engine type
    I,  // WASM instance type
    R,  // Fee reserve type
> where
    W: WasmEngine<I>,
    I: WasmInstance,
    R: FeeReserve,
{
    /// Current execution mode, specifies permissions into state/invocations
    execution_mode: ExecutionMode,

    /// The transaction hash
    transaction_hash: Hash,
    /// Blobs attached to the transaction
    blobs: &'g HashMap<Hash, Vec<u8>>,

    /// State
    heap: &'g mut Heap,
    track: &'g mut Track<'s, R>,
    /// ID allocator
    id_allocator: IdAllocator,

    /// Interpreter capable of running scrypto programs
    scrypto_interpreter: &'g mut ScryptoInterpreter<I, W>,

    /// Call frames
    current_frame: CallFrame,
    // This stack could potentially be removed and just use the native stack
    // but keeping this call_frames stack may potentially prove useful if implementing
    // execution pause and/or better debuggability
    prev_frame_stack: Vec<CallFrame>,

    /// Kernel modules
    modules: Vec<Box<dyn Module<R>>>,
    /// The max call depth, TODO: Move into costing module
    max_depth: usize,
}

impl<'g, 's, W, I, R> Kernel<'g, 's, W, I, R>
where
    W: WasmEngine<I>,
    I: WasmInstance,
    R: FeeReserve,
{
    pub fn new(
        transaction_hash: Hash,
        auth_zone_params: AuthZoneParams,
        blobs: &'g HashMap<Hash, Vec<u8>>,
        max_depth: usize,
        heap: &'g mut Heap,
        track: &'g mut Track<'s, R>,
        scrypto_interpreter: &'g mut ScryptoInterpreter<I, W>,
        modules: Vec<Box<dyn Module<R>>>,
    ) -> Self {
        let frame = CallFrame::new_root();
        let mut kernel = Self {
            execution_mode: ExecutionMode::Kernel,
            transaction_hash,
            blobs,
            max_depth,
            heap,
            track,
            scrypto_interpreter,
            id_allocator: IdAllocator::new(IdSpace::Application),
            current_frame: frame,
            prev_frame_stack: vec![],
            modules,
        };

        // Initial authzone
        // TODO: Move into module initialization
        let virtualizable_proofs_resource_addresses =
            auth_zone_params.virtualizable_proofs_resource_addresses;
        let mut proofs_to_create = BTreeMap::<ResourceAddress, BTreeSet<NonFungibleId>>::new();
        for non_fungible in auth_zone_params.initial_proofs {
            proofs_to_create
                .entry(non_fungible.resource_address())
                .or_insert(BTreeSet::new())
                .insert(non_fungible.non_fungible_id());
        }
        let mut proofs = Vec::new();

        for (resource_address, non_fungible_ids) in proofs_to_create {
            let bucket_id =
                kernel.create_non_fungible_bucket_with_ids(resource_address, non_fungible_ids);

            let proof = kernel
                .execute_in_mode::<_, _, RuntimeError>(ExecutionMode::AuthModule, |system_api| {
                    let node_id = RENodeId::Bucket(bucket_id);
                    let offset = SubstateOffset::Bucket(BucketOffset::Bucket);
                    let handle = system_api.lock_substate(node_id, offset, LockFlags::MUTABLE)?;
                    let mut substate_mut = system_api.get_ref_mut(handle)?;
                    let mut raw_mut = substate_mut.get_raw_mut();
                    let proof = raw_mut
                        .bucket()
                        .create_proof(bucket_id)
                        .expect("Failed to create proof");
                    substate_mut.flush()?;
                    Ok(proof)
                })
                .unwrap();

            proofs.push(proof);
        }

        // Create empty buckets for virtual proofs
        let mut virtual_proofs_buckets: BTreeMap<ResourceAddress, BucketId> = BTreeMap::new();
        for resource_address in virtualizable_proofs_resource_addresses {
            let bucket_id = kernel
                .create_non_fungible_bucket_with_ids(resource_address.clone(), BTreeSet::new());
            virtual_proofs_buckets.insert(resource_address, bucket_id);
        }

        let auth_zone = AuthZoneStackSubstate::new(proofs, virtual_proofs_buckets);

        kernel
            .create_node(RENode::AuthZone(auth_zone))
            .expect("Failed to create AuthZone");

        kernel
    }

    fn create_non_fungible_bucket_with_ids(
        &mut self,
        resource_address: ResourceAddress,
        ids: BTreeSet<NonFungibleId>,
    ) -> BucketId {
        match self
            .create_node(RENode::Bucket(BucketSubstate::new(
                Resource::new_non_fungible(resource_address, ids),
            )))
            .expect("Failed to create a bucket")
        {
            RENodeId::Bucket(bucket_id) => bucket_id,
            _ => panic!("Expected Bucket RENodeId but received something else"),
        }
    }

    fn new_uuid(
        id_allocator: &mut IdAllocator,
        transaction_hash: Hash,
    ) -> Result<u128, IdAllocationError> {
        id_allocator.new_uuid(transaction_hash)
    }

    // TODO: Move this into a native function
    fn create_global_node(&mut self, node_id: RENodeId) -> Result<(GlobalAddress, GlobalAddressSubstate), RuntimeError> {
        self.execute_in_mode(ExecutionMode::Globalize, |system_api| {
            match node_id {
                RENodeId::Component(component_id) => {
                    let transaction_hash = system_api.transaction_hash;
                    let handle = system_api.lock_substate(node_id, SubstateOffset::Component(ComponentOffset::Info), LockFlags::read_only())?;
                    let substate_ref = system_api.get_ref(handle)?;
                    let info = substate_ref.component_info();
                    let (package_address, blueprint_name) = (info.package_address, info.blueprint_name.clone());
                    system_api.drop_lock(handle)?;

                    let component_address = system_api.id_allocator
                        .new_component_address(
                            transaction_hash,
                            package_address,
                            &blueprint_name,
                        )
                        .map_err(|e| RuntimeError::KernelError(KernelError::IdAllocationError(e)))?;

                    Ok((
                        GlobalAddress::Component(component_address),
                        GlobalAddressSubstate::Component(scrypto::component::Component(component_id)),
                    ))
                }
                RENodeId::System(component_id) => {
                    let transaction_hash = system_api.transaction_hash;

                    let component_address = system_api.id_allocator
                        .new_system_component_address(transaction_hash)
                        .map_err(|e| RuntimeError::KernelError(KernelError::IdAllocationError(e)))?;

                    Ok((
                        GlobalAddress::Component(component_address),
                        GlobalAddressSubstate::SystemComponent(scrypto::component::Component(
                            component_id,
                        )),
                    ))
                }
                RENodeId::ResourceManager(resource_address) => {
                    Ok((
                        GlobalAddress::Resource(resource_address),
                        GlobalAddressSubstate::Resource(resource_address),
                    ))
                }
                RENodeId::Package(package_address) => {
                    Ok((
                        GlobalAddress::Package(package_address),
                        GlobalAddressSubstate::Package(package_address),
                    ))
                }
                _ => {
                    Err(RuntimeError::KernelError(
                        KernelError::RENodeGlobalizeTypeNotAllowed(node_id),
                    ))
                }
            }
        })
    }

    fn new_node_id(
        id_allocator: &mut IdAllocator,
        transaction_hash: Hash,
        re_node: &RENode,
    ) -> Result<RENodeId, IdAllocationError> {
        match re_node {
            RENode::Global(..) => panic!("Should not get here"),
            RENode::AuthZone(..) => {
                let auth_zone_id = id_allocator.new_auth_zone_id()?;
                Ok(RENodeId::AuthZoneStack(auth_zone_id))
            }
            RENode::Bucket(..) => {
                let bucket_id = id_allocator.new_bucket_id()?;
                Ok(RENodeId::Bucket(bucket_id))
            }
            RENode::Proof(..) => {
                let proof_id = id_allocator.new_proof_id()?;
                Ok(RENodeId::Proof(proof_id))
            }
            RENode::Worktop(..) => Ok(RENodeId::Worktop),
            RENode::Vault(..) => {
                let vault_id = id_allocator.new_vault_id(transaction_hash)?;
                Ok(RENodeId::Vault(vault_id))
            }
            RENode::KeyValueStore(..) => {
                let kv_store_id = id_allocator.new_kv_store_id(transaction_hash)?;
                Ok(RENodeId::KeyValueStore(kv_store_id))
            }
            RENode::NonFungibleStore(..) => {
                let non_fungible_store_id =
                    id_allocator.new_non_fungible_store_id(transaction_hash)?;
                Ok(RENodeId::NonFungibleStore(non_fungible_store_id))
            }
            RENode::Package(..) => {
                // Security Alert: ensure ID allocating will practically never fail
                let package_address = id_allocator.new_package_address(transaction_hash)?;
                Ok(RENodeId::Package(package_address))
            }
            RENode::ResourceManager(..) => {
                let resource_address = id_allocator.new_resource_address(transaction_hash)?;
                Ok(RENodeId::ResourceManager(resource_address))
            }
            RENode::Component(..) => {
                let component_id = id_allocator.new_component_id(transaction_hash)?;
                Ok(RENodeId::Component(component_id))
            }
            RENode::System(..) => {
                let component_id = id_allocator.new_component_id(transaction_hash)?;
                Ok(RENodeId::System(component_id))
            }
        }
    }

    fn run(
        &mut self,
        actor: REActor,
        input: ScryptoValue,
        nodes_to_pass: Vec<RENodeId>,
        mut refed_nodes: HashMap<RENodeId, RENodePointer>,
    ) -> Result<ScryptoValue, RuntimeError> {
        let new_refed_nodes = self.execute_in_mode(ExecutionMode::AuthModule, |system_api| {
            AuthModule::on_before_frame_start(&actor, &input, system_api).map_err(|e| match e {
                InvokeError::Error(e) => RuntimeError::ModuleError(e.into()),
                InvokeError::Downstream(runtime_error) => runtime_error,
            })
        })?;

        // TODO: Do this in a better way by allowing module to execute in next call frame
        for new_refed_node in new_refed_nodes {
            let node_pointer = self.current_frame
                .get_node_pointer(new_refed_node)
                .unwrap();
            refed_nodes.insert(new_refed_node, node_pointer);
        }

        // Call Frame Push
        let frame = CallFrame::new_child_from_parent(
            &mut self.heap,
            &mut self.current_frame,
            actor,
            nodes_to_pass,
            refed_nodes,
        )?;

        let parent = mem::replace(&mut self.current_frame, frame);
        self.prev_frame_stack.push(parent);

        // Execute
        let actor = self.current_frame.actor.clone();
        let output = match actor.clone() {
            REActor::Function(ResolvedFunction::Native(native_fn)) => self
                .execute_in_mode(ExecutionMode::Application, |system_api| {
                    NativeInterpreter::run_function(native_fn, input, system_api)
                }),
            REActor::Method(ResolvedReceiverMethod {
                receiver,
                method: ResolvedMethod::Native(native_method),
            }) => self.execute_in_mode(ExecutionMode::Application, |system_api| {
                NativeInterpreter::run_method(receiver.receiver(), native_method, input, system_api)
            }),
            REActor::Function(ResolvedFunction::Scrypto {
                package_address, ..
            })
            | REActor::Method(ResolvedReceiverMethod {
                method:
                    ResolvedMethod::Scrypto {
                        package_address, ..
                    },
                ..
            }) => {
                // TODO: Move into interpreter when interpreter trait implemented
                let package = self.execute_in_mode::<_, _, RuntimeError>(
                    ExecutionMode::ScryptoInterpreter,
                    |system_api| {
                        let package_id = RENodeId::Global(GlobalAddress::Package(package_address));
                        let package_offset = SubstateOffset::Package(PackageOffset::Package);
                        let handle = system_api.lock_substate(
                            package_id,
                            package_offset,
                            LockFlags::read_only(),
                        )?;
                        let substate_ref = system_api.get_ref(handle)?;
                        let package = substate_ref.package().clone();
                        system_api.drop_lock(handle)?;
                        Ok(package)
                    },
                )?;
                let mut executor = self.scrypto_interpreter.create_executor(package);

                self.execute_in_mode(ExecutionMode::ScryptoInterpreter, |system_api| {
                    executor.run(input, system_api)
                })
            }
        }?;

        // Process return data
        let mut parent = self.prev_frame_stack.pop().unwrap();
        CallFrame::move_nodes_upstream(&mut self.heap, &mut self.current_frame, &mut parent, output.node_ids())?;
        CallFrame::copy_refs(&mut self.current_frame, &mut parent, output.global_references())?;

        // Auto drop locks
        for (_, lock) in self.current_frame.drain_locks() {
            let SubstateLock {
                substate_pointer: (node_pointer, offset),
                flags,
                ..
            } = lock;
            if !(matches!(offset, SubstateOffset::KeyValueStore(..))
                || matches!(
                    offset,
                    SubstateOffset::NonFungibleStore(NonFungibleStoreOffset::Entry(..))
                ))
            {
                node_pointer
                    .release_lock(
                        offset,
                        flags.contains(LockFlags::UNMODIFIED_BASE),
                        self.track,
                    )
                    .map_err(RuntimeError::KernelError)?;
            }
        }

        // TODO: Auto drop locks of module execution as well
        self.execute_in_mode(ExecutionMode::AuthModule, |system_api| {
            AuthModule::on_frame_end(system_api).map_err(|e| match e {
                InvokeError::Error(e) => RuntimeError::ModuleError(e.into()),
                InvokeError::Downstream(runtime_error) => runtime_error,
            })
        })?;

        // drop proofs and check resource leak
        {
            let mut child = mem::replace(&mut self.current_frame, parent);
            child.drop_frame(&mut self.heap)?;
        }

        Ok(output)
    }

    pub fn node_method_deref(
        &mut self,
        node_id: RENodeId,
    ) -> Result<Option<RENodeId>, RuntimeError> {
        if let RENodeId::Global(..) = node_id {
            let node_id =
                self.execute_in_mode::<_, _, RuntimeError>(ExecutionMode::Deref, |system_api| {
                    let offset = SubstateOffset::Global(GlobalOffset::Global);
                    let handle = system_api.lock_substate(node_id, offset, LockFlags::empty())?;
                    let substate_ref = system_api.get_ref(handle)?;
                    Ok(substate_ref.global_address().node_deref())
                })?;

            Ok(Some(node_id))
        } else {
            Ok(None)
        }
    }

    pub fn node_offset_deref(
        &mut self,
        node_id: RENodeId,
        offset: &SubstateOffset,
    ) -> Result<Option<RENodeId>, RuntimeError> {
        if let RENodeId::Global(..) = node_id {
            if !matches!(offset, SubstateOffset::Global(GlobalOffset::Global)) {
                let node_id = self.execute_in_mode::<_, _, RuntimeError>(
                    ExecutionMode::Deref,
                    |system_api| {
                        let handle = system_api.lock_substate(
                            node_id,
                            SubstateOffset::Global(GlobalOffset::Global),
                            LockFlags::empty(),
                        )?;
                        let substate_ref = system_api.get_ref(handle)?;
                        Ok(substate_ref.global_address().node_deref())
                    },
                )?;

                Ok(Some(node_id))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn resolve_method_actor(
        &mut self,
        receiver: ResolvedReceiver,
        method_ident: MethodIdent,
        input: &ScryptoValue,
    ) -> Result<(REActor, HashMap<RENodeId, RENodePointer>), RuntimeError> {
        let mut references_to_add = HashMap::new();

        let actor = match &method_ident {
            MethodIdent::Scrypto(ident) => {
                let (actor, node_id) =
                    self.execute_in_mode(ExecutionMode::ScryptoInterpreter, |system_api| {
                        ScryptoInterpreter::<I, W>::load_scrypto_actor(
                            ScryptoFnIdent::Method(receiver.clone(), ident.clone()),
                            input,
                            system_api,
                        )
                        .map_err(|e| match e {
                            InvokeError::Downstream(runtime_error) => runtime_error,
                            InvokeError::Error(error) => RuntimeError::InterpreterError(
                                InterpreterError::InvalidScryptoMethod(
                                    receiver.clone(),
                                    method_ident.clone(),
                                    error,
                                ),
                            ),
                        })
                    })?;

                // TODO: Move this in a better spot when more refactors are done
                let package = self.execute_in_mode::<_, _, RuntimeError>(
                    ExecutionMode::ScryptoInterpreter,
                    |system_api| {
                        let handle = system_api.lock_substate(
                            node_id,
                            SubstateOffset::Package(PackageOffset::Package),
                            LockFlags::read_only(),
                        )?;
                        let substate_ref = system_api.get_ref(handle)?;
                        let package = substate_ref.package().clone(); // TODO: Remove clone()
                        system_api.drop_lock(handle)?;
                        Ok(package)
                    },
                )?;
                for m in &mut self.modules {
                    m.on_wasm_instantiation(
                        &self.current_frame,
                        &mut self.heap,
                        &mut self.track,
                        package.code(),
                    )
                    .map_err(RuntimeError::ModuleError)?;
                }

                let node_pointer = self.current_frame
                    .get_node_pointer(node_id)
                    .unwrap();
                references_to_add.insert(node_id, node_pointer);
                actor
            }
            MethodIdent::Native(native_fn) => REActor::Method(ResolvedReceiverMethod {
                receiver: receiver.clone(),
                method: ResolvedMethod::Native(native_fn.clone()),
            }),
        };

        Ok((actor, references_to_add))
    }

    fn resolve_function_actor(
        &mut self,
        function_ident: FunctionIdent,
        input: &ScryptoValue,
    ) -> Result<(REActor, HashMap<RENodeId, RENodePointer>), RuntimeError> {
        let mut references_to_add = HashMap::new();

        let actor = match &function_ident {
            FunctionIdent::Scrypto {
                package_address,
                blueprint_name,
                ident,
            } => {
                let (actor, node_id) =
                    self.execute_in_mode(ExecutionMode::ScryptoInterpreter, |system_api| {
                        ScryptoInterpreter::<I, W>::load_scrypto_actor(
                            ScryptoFnIdent::Function(
                                *package_address,
                                blueprint_name.clone(),
                                ident.to_string(),
                            ),
                            input,
                            system_api,
                        )
                        .map_err(|e| match e {
                            InvokeError::Downstream(runtime_error) => runtime_error,
                            InvokeError::Error(error) => RuntimeError::InterpreterError(
                                InterpreterError::InvalidScryptoFunction(
                                    function_ident.clone(),
                                    error,
                                ),
                            ),
                        })
                    })?;

                // TODO: Move this in a better spot when more refactors are done
                let package = self.execute_in_mode::<_, _, RuntimeError>(
                    ExecutionMode::ScryptoInterpreter,
                    |system_api| {
                        let handle = system_api.lock_substate(
                            node_id,
                            SubstateOffset::Package(PackageOffset::Package),
                            LockFlags::read_only(),
                        )?;
                        let substate_ref = system_api.get_ref(handle)?;
                        let package = substate_ref.package().clone(); // TODO: Remove clone()
                        system_api.drop_lock(handle)?;
                        Ok(package)
                    },
                )?;
                for m in &mut self.modules {
                    m.on_wasm_instantiation(
                        &self.current_frame,
                        &mut self.heap,
                        &mut self.track,
                        package.code(),
                    )
                    .map_err(RuntimeError::ModuleError)?;
                }

                let node_pointer = self.current_frame
                    .get_node_pointer(node_id)
                    .unwrap();
                references_to_add.insert(node_id, node_pointer);
                actor
            }
            FunctionIdent::Native(native_function) => {
                REActor::Function(ResolvedFunction::Native(native_function.clone()))
            }
        };

        Ok((actor, references_to_add))
    }

    fn verify_valid_mode_transition(
        cur: &ExecutionMode,
        next: &ExecutionMode,
    ) -> Result<(), RuntimeError> {
        match (cur, next) {
            (ExecutionMode::Kernel, ..) => Ok(()),
            (ExecutionMode::ScryptoInterpreter, ExecutionMode::Application) => Ok(()),
            _ => Err(RuntimeError::KernelError(
                KernelError::InvalidModeTransition(*cur, *next),
            )),
        }
    }
}

impl<'g, 's, W, I, R> SystemApi<'s, R> for Kernel<'g, 's, W, I, R>
where
    W: WasmEngine<I>,
    I: WasmInstance,
    R: FeeReserve,
{
    fn execute_in_mode<X, RTN, E>(
        &mut self,
        execution_mode: ExecutionMode,
        execute: X,
    ) -> Result<RTN, RuntimeError>
    where
        RuntimeError: From<E>,
        X: FnOnce(&mut Self) -> Result<RTN, E>,
    {
        Self::verify_valid_mode_transition(&self.execution_mode, &execution_mode)?;

        // Save and replace kernel actor
        let saved = self.execution_mode;
        self.execution_mode = execution_mode;

        let rtn = execute(self)?;

        // Restore old kernel actor
        self.execution_mode = saved;

        Ok(rtn)
    }

    fn consume_cost_units(&mut self, units: u32) -> Result<(), RuntimeError> {
        for m in &mut self.modules {
            m.on_wasm_costing(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                units,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(())
    }

    fn lock_fee(
        &mut self,
        vault_id: VaultId,
        mut fee: Resource,
        contingent: bool,
    ) -> Result<Resource, RuntimeError> {
        for m in &mut self.modules {
            fee = m
                .on_lock_fee(
                    &self.current_frame,
                    &mut self.heap,
                    &mut self.track,
                    vault_id,
                    fee,
                    contingent,
                )
                .map_err(RuntimeError::ModuleError)?;
        }

        Ok(fee)
    }

    fn get_actor(&self) -> &REActor {
        &self.current_frame.actor
    }

    fn invoke(
        &mut self,
        fn_ident: FnIdent,
        input: ScryptoValue,
    ) -> Result<ScryptoValue, RuntimeError> {
        let depth = self.current_frame.depth;

        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::Invoke {
                    fn_ident: &fn_ident,
                    input: &input,
                    depth,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        // check call depth
        if depth == self.max_depth {
            return Err(RuntimeError::KernelError(
                KernelError::MaxCallDepthLimitReached,
            ));
        }

        let mut nodes_to_pass_downstream = Vec::new();
        let mut next_node_refs = HashMap::new();

        nodes_to_pass_downstream.extend(input.node_ids());
        // Internal state update to taken values

        // Move this into higher layer, e.g. transaction processor
        if self.current_frame.depth == 0 {
            let mut static_refs = HashSet::new();
            static_refs.insert(GlobalAddress::Resource(RADIX_TOKEN));
            static_refs.insert(GlobalAddress::Resource(SYSTEM_TOKEN));
            static_refs.insert(GlobalAddress::Resource(ECDSA_SECP256K1_TOKEN));
            static_refs.insert(GlobalAddress::Component(SYS_SYSTEM_COMPONENT));
            static_refs.insert(GlobalAddress::Package(ACCOUNT_PACKAGE));
            static_refs.insert(GlobalAddress::Package(SYS_FAUCET_PACKAGE));

            // Make refs visible
            let mut global_references = input.global_references();
            global_references.extend(static_refs.clone());

            // TODO: This can be refactored out once any type in sbor is implemented
            let maybe_txn: Result<TransactionProcessorRunInput, DecodeError> =
                scrypto_decode(&input.raw);
            if let Ok(input) = maybe_txn {
                for instruction in &input.instructions {
                    match instruction {
                        Instruction::CallFunction { args, .. }
                        | Instruction::CallMethod { args, .. } => {
                            let scrypto_value =
                                ScryptoValue::from_slice(&args).expect("Invalid CALL arguments");
                            global_references.extend(scrypto_value.global_references());
                        }
                        _ => {}
                    }
                }
            }

            // Check for existence
            for global_address in global_references {
                let node_id = RENodeId::Global(global_address);
                let offset = SubstateOffset::Global(GlobalOffset::Global);
                let node_pointer = RENodePointer::Store(node_id);

                // TODO: static check here is to support the current genesis transaction which
                // TODO: requires references to dynamically created resources. Can remove
                // TODO: when this is resolved.
                if !static_refs.contains(&global_address) {
                    node_pointer
                        .acquire_lock(offset.clone(), LockFlags::read_only(), &mut self.track)
                        .map_err(|e| match e {
                            KernelError::TrackError(TrackError::NotFound(..)) => {
                                RuntimeError::KernelError(KernelError::GlobalAddressNotFound(
                                    global_address,
                                ))
                            }
                            _ => RuntimeError::KernelError(e),
                        })?;
                    node_pointer
                        .release_lock(offset, false, &mut self.track)
                        .map_err(RuntimeError::KernelError)?;
                }

                self.current_frame
                    .node_refs
                    .insert(node_id, node_pointer);
                next_node_refs.insert(node_id, node_pointer);
            }
        } else {
            // Check that global references are owned by this call frame
            let mut global_references = input.global_references();
            global_references.insert(GlobalAddress::Resource(RADIX_TOKEN));
            global_references.insert(GlobalAddress::Component(SYS_SYSTEM_COMPONENT));
            for global_address in global_references {
                let node_id = RENodeId::Global(global_address);

                // As of now, once a component is made visible to the frame, client can directly
                // read the substates of the component. This will cause "Substate was never locked" issue.
                // We use the following temporary solution to work around this.
                // A better solution is to create node representation before issuing any reference.
                // TODO: remove
                if let Some(pointer) = self.current_frame
                    .node_refs
                    .get(&node_id)
                {
                    next_node_refs.insert(node_id.clone(), pointer.clone());
                } else {
                    return Err(RuntimeError::KernelError(
                        KernelError::InvalidReferencePass(global_address),
                    ));
                }
            }
        }

        // Change to kernel mode
        let saved_mode = self.execution_mode;
        self.execution_mode = ExecutionMode::Kernel;

        let (next_actor, references_to_add) = match fn_ident {
            FnIdent::Method(ReceiverMethodIdent {
                receiver,
                method_ident,
            }) => {
                let resolved_receiver = match receiver {
                    Receiver::Consumed(node_id) => {
                        nodes_to_pass_downstream.push(node_id);
                        ResolvedReceiver::new(Receiver::Consumed(node_id))
                    }
                    Receiver::Ref(node_id) => {
                        // Deref
                        let resolved_receiver =
                            if let Some(derefed) = self.node_method_deref(node_id)? {
                                ResolvedReceiver::derefed(Receiver::Ref(derefed), node_id)
                            } else {
                                ResolvedReceiver::new(Receiver::Ref(node_id))
                            };

                        let resolved_node_id = resolved_receiver.node_id();
                        let node_pointer = self.current_frame.get_node_pointer(resolved_node_id)?;
                        next_node_refs.insert(resolved_node_id, node_pointer);

                        resolved_receiver
                    }
                };
                self.resolve_method_actor(resolved_receiver, method_ident, &input)?
            }
            FnIdent::Function(function_ident) => {
                self.resolve_function_actor(function_ident, &input)?
            }
        };
        next_node_refs.extend(references_to_add);

        let output = self.run(next_actor, input, nodes_to_pass_downstream, next_node_refs)?;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::Invoke { output: &output },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        // TODO: Move this into higher layer, e.g. transaction processor
        if self.current_frame.depth == 0 {
            self.current_frame.drop_frame(&mut self.heap)?;
        }

        // Restore previous mode
        self.execution_mode = saved_mode;

        Ok(output)
    }

    fn get_visible_node_ids(&mut self) -> Result<Vec<RENodeId>, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::ReadOwnedNodes,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        let node_ids = self.current_frame.get_visible_nodes();

        Ok(node_ids)
    }

    fn drop_node(&mut self, node_id: RENodeId) -> Result<HeapRENode, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::DropNode { node_id: &node_id },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        // TODO: Authorization

        let node = self.current_frame.drop_node(self.heap, node_id)?;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::DropNode { node: &node },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(node)
    }

    fn create_node(&mut self, re_node: RENode) -> Result<RENodeId, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::CreateNode { node: &re_node },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        // TODO: Authorization

        let node_id = Self::new_node_id(&mut self.id_allocator, self.transaction_hash, &re_node)
            .map_err(|e| RuntimeError::KernelError(KernelError::IdAllocationError(e)))?;
        self.current_frame.create_node(
            &mut self.heap,
            node_id,
            re_node,
        )?;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::CreateNode { node_id: &node_id },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(node_id)
    }

    fn node_globalize(&mut self, node_id: RENodeId) -> Result<GlobalAddress, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::GlobalizeNode { node_id: &node_id },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        // Change to kernel mode
        let current_mode = self.execution_mode;
        self.execution_mode = ExecutionMode::Kernel;

        // TODO: Authorization

        let (global_address, global_substate) = self.create_global_node(node_id)?;
        self.track.new_global_addresses.push(global_address);
        self.track.insert_substate(
            SubstateId(
                RENodeId::Global(global_address),
                SubstateOffset::Global(GlobalOffset::Global),
            ),
            RuntimeSubstate::GlobalRENode(global_substate),
        );
        self.current_frame
            .node_refs
            .insert(
                RENodeId::Global(global_address),
                RENodePointer::Store(RENodeId::Global(global_address)),
            );
        self.current_frame.move_owned_node_to_store(&mut self.heap, &mut self.track, node_id)?;

        // Restore current mode
        self.execution_mode = current_mode;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::GlobalizeNode,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(global_address)
    }

    fn lock_substate(
        &mut self,
        mut node_id: RENodeId,
        offset: SubstateOffset,
        flags: LockFlags,
    ) -> Result<LockHandle, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::LockSubstate {
                    node_id: &node_id,
                    offset: &offset,
                    flags: &flags,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        // Change to kernel mode
        let current_mode = self.execution_mode;
        self.execution_mode = ExecutionMode::Kernel;

        // Deref
        if let Some(derefed) = self.node_offset_deref(node_id, &offset)? {
            node_id = derefed;
        }

        let node_pointer = self.current_frame.get_node_pointer(node_id)?;

        // TODO: Check if valid offset for node_id

        // Authorization
        let actor = &self.current_frame.actor;
        if !SubstateProperties::check_substate_access(
            current_mode,
            actor,
            node_id,
            offset.clone(),
            flags,
        ) {
            return Err(RuntimeError::KernelError(
                KernelError::InvalidSubstateLock {
                    mode: current_mode,
                    actor: actor.clone(),
                    node_id,
                    offset,
                    flags,
                },
            ));
        }

        if !(matches!(offset, SubstateOffset::KeyValueStore(..))
            || matches!(
                offset,
                SubstateOffset::NonFungibleStore(NonFungibleStoreOffset::Entry(..))
            ))
        {
            node_pointer
                .acquire_lock(offset.clone(), flags, &mut self.track)
                .map_err(RuntimeError::KernelError)?;
        }

        let lock_handle = self.current_frame.create_lock(
            node_pointer,
            offset.clone(),
            flags,
        );

        // Restore current mode
        self.execution_mode = current_mode;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::LockSubstate { lock_handle },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(lock_handle)
    }

    fn drop_lock(&mut self, lock_handle: LockHandle) -> Result<(), RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::DropLock {
                    lock_handle: &lock_handle,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        let (node_pointer, offset, flags) = self.current_frame
            .drop_lock(lock_handle)
            .map_err(RuntimeError::KernelError)?;

        if !(matches!(offset, SubstateOffset::KeyValueStore(..))
            || matches!(
                offset,
                SubstateOffset::NonFungibleStore(NonFungibleStoreOffset::Entry(..))
            ))
        {
            node_pointer
                .release_lock(
                    offset.clone(),
                    flags.contains(LockFlags::UNMODIFIED_BASE),
                    self.track,
                )
                .map_err(RuntimeError::KernelError)?;
        }

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::DropLock,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(())
    }

    fn get_ref(&mut self, lock_handle: LockHandle) -> Result<SubstateRef, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::GetRef {
                    lock_handle: &lock_handle,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        let SubstateLock {
            substate_pointer: (node_pointer, offset),
            ..
        } = self.current_frame
            .get_lock(lock_handle)
            .map_err(RuntimeError::KernelError)?
            .clone();

        let (global_references, children) = {
            let substate_ref =
                node_pointer.borrow_substate(&offset, &mut self.heap, &mut self.track)?;
            substate_ref.references_and_owned_nodes()
        };

        // Expand references
        {
            // TODO: Figure out how to drop these references as well on reference drop
            for global_address in global_references {
                let node_id = RENodeId::Global(global_address);
                self.current_frame
                    .node_refs
                    .insert(node_id, RENodePointer::Store(node_id));
            }
            for child_id in children {
                let child_pointer = node_pointer.child(child_id);
                self.current_frame.node_refs.insert(child_id, child_pointer);
                self.current_frame
                    .add_lock_visible_node(lock_handle, child_id)
                    .map_err(RuntimeError::KernelError)?;
            }
        }

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::GetRef {
                    node_pointer: &node_pointer,
                    offset: &offset,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        node_pointer.borrow_substate(&offset, &mut self.heap, &mut self.track)
    }

    fn get_ref_mut<'f>(
        &'f mut self,
        lock_handle: LockHandle,
    ) -> Result<SubstateRefMut<'f, 's, R>, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::GetRefMut {
                    lock_handle: &lock_handle,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        let SubstateLock {
            substate_pointer: (node_pointer, offset),
            flags,
            ..
        } = self.current_frame
            .get_lock(lock_handle)
            .map_err(RuntimeError::KernelError)?
            .clone();

        if !flags.contains(LockFlags::MUTABLE) {
            return Err(RuntimeError::KernelError(KernelError::LockNotMutable(
                lock_handle,
            )));
        }

        let (global_references, children) = {
            let substate_ref =
                node_pointer.borrow_substate(&offset, &mut self.heap, &mut self.track)?;
            substate_ref.references_and_owned_nodes()
        };

        // Expand references
        {
            // TODO: Figure out how to drop these references as well on reference drop
            for global_address in global_references {
                let node_id = RENodeId::Global(global_address);
                self.current_frame
                    .node_refs
                    .insert(node_id, RENodePointer::Store(node_id));
            }
            for child_id in &children {
                let child_pointer = node_pointer.child(*child_id);
                self.current_frame.node_refs.insert(*child_id, child_pointer);
                self.current_frame
                    .add_lock_visible_node(lock_handle, *child_id)
                    .map_err(RuntimeError::KernelError)?;
            }
        }

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::GetRefMut,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        SubstateRefMut::new(
            lock_handle,
            node_pointer,
            offset,
            children,
            &mut self.current_frame,
            &mut self.heap,
            &mut self.track,
        )
    }

    fn read_transaction_hash(&mut self) -> Result<Hash, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::ReadTransactionHash,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::ReadTransactionHash {
                    hash: &self.transaction_hash,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(self.transaction_hash)
    }

    fn read_blob(&mut self, blob_hash: &Hash) -> Result<&[u8], RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::ReadBlob { blob_hash },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        let blob = self
            .blobs
            .get(blob_hash)
            .ok_or(KernelError::BlobNotFound(blob_hash.clone()))
            .map_err(RuntimeError::KernelError)?;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::ReadBlob { blob: &blob },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(blob)
    }

    fn generate_uuid(&mut self) -> Result<u128, RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::GenerateUuid,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        let uuid = Self::new_uuid(&mut self.id_allocator, self.transaction_hash)
            .map_err(|e| RuntimeError::KernelError(KernelError::IdAllocationError(e)))?;

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::GenerateUuid { uuid },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(uuid)
    }

    fn emit_log(&mut self, level: Level, message: String) -> Result<(), RuntimeError> {
        for m in &mut self.modules {
            m.pre_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallInput::EmitLog {
                    level: &level,
                    message: &message,
                },
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        self.track.add_log(level, message);

        for m in &mut self.modules {
            m.post_sys_call(
                &self.current_frame,
                &mut self.heap,
                &mut self.track,
                SysCallOutput::EmitLog,
            )
            .map_err(RuntimeError::ModuleError)?;
        }

        Ok(())
    }
}
