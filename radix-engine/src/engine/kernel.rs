use core::marker::PhantomData;

use sbor::rust::boxed::Box;
use sbor::rust::collections::*;
use sbor::rust::format;
use sbor::rust::string::String;
use sbor::rust::string::ToString;
use sbor::rust::vec;
use sbor::*;
use scrypto::buffer::scrypto_decode;
use scrypto::core::Receiver;
use scrypto::core::{ScryptoActor, TypeName};
use scrypto::engine::types::*;
use scrypto::values::*;
use transaction::model::ExecutableInstruction;
use transaction::validation::*;

use crate::engine::*;
use crate::fee::*;
use crate::model::*;
use crate::wasm::*;

#[macro_export]
macro_rules! trace {
    ( $self: expr, $level: expr, $msg: expr $( , $arg:expr )* ) => {
        #[cfg(not(feature = "alloc"))]
        if $self.trace {
            println!("{}[{:5}] {}", "  ".repeat($self.call_frames.len() - 1) , $level, sbor::rust::format!($msg, $( $arg ),*));
        }
    };
}

pub struct Kernel<
    'g, // Lifetime of values outliving all frames
    's, // Substate store lifetime
    W,  // WASM engine type
    I,  // WASM instance type
    C,  // Fee reserve type
> where
    W: WasmEngine<I>,
    I: WasmInstance,
    C: FeeReserve,
{
    /// The transaction hash
    transaction_hash: Hash,
    /// The max call depth
    max_depth: usize,
    /// Whether to show trace messages
    #[allow(dead_code)] // for no_std
    trace: bool,

    /// State track
    track: &'g mut Track<'s>,
    /// Wasm engine
    wasm_engine: &'g mut W,
    /// Wasm Instrumenter
    wasm_instrumenter: &'g mut WasmInstrumenter,

    /// Fee reserve
    fee_reserve: &'g mut C,
    /// Fee table
    fee_table: &'g FeeTable,

    /// ID allocator
    id_allocator: IdAllocator,
    /// Call frames
    call_frames: Vec<CallFrame>,

    phantom: PhantomData<I>,
}

impl<'g, 's, W, I, C> Kernel<'g, 's, W, I, C>
where
    W: WasmEngine<I>,
    I: WasmInstance,
    C: FeeReserve,
{
    pub fn new(
        transaction_hash: Hash,
        transaction_signers: Vec<EcdsaPublicKey>,
        is_system: bool,
        max_depth: usize,
        trace: bool,
        track: &'g mut Track<'s>,
        wasm_engine: &'g mut W,
        wasm_instrumenter: &'g mut WasmInstrumenter,
        fee_reserve: &'g mut C,
        fee_table: &'g FeeTable,
    ) -> Self {
        let mut kernel = Self {
            transaction_hash,
            max_depth,
            trace,
            track,
            wasm_engine,
            wasm_instrumenter,
            fee_reserve,
            fee_table,
            id_allocator: IdAllocator::new(IdSpace::Application),
            call_frames: vec![],
            phantom: PhantomData,
        };

        let frame = CallFrame::new_root(transaction_signers, is_system, &mut kernel);
        kernel.call_frames.push(frame);

        kernel
    }

    fn process_call_data(validated: &ScryptoValue) -> Result<(), RuntimeError> {
        if !validated.kv_store_ids.is_empty() {
            return Err(RuntimeError::KeyValueStoreNotAllowed);
        }
        if !validated.vault_ids.is_empty() {
            return Err(RuntimeError::VaultNotAllowed);
        }
        Ok(())
    }

    fn process_return_data(validated: &ScryptoValue) -> Result<(), RuntimeError> {
        if !validated.kv_store_ids.is_empty() {
            return Err(RuntimeError::KeyValueStoreNotAllowed);
        }

        // TODO: Should we disallow vaults to be moved?

        Ok(())
    }

    fn read_value_internal(
        call_frames: &mut Vec<CallFrame>,
        track: &mut Track<'s>,
        substate_id: &SubstateId,
    ) -> Result<(RENodePointer, ScryptoValue), RuntimeError> {
        let node_id = SubstateProperties::get_node_id(substate_id);

        // Get location
        // Note this must be run AFTER values are taken, otherwise there would be inconsistent readable_values state
        let node_pointer = call_frames
            .last()
            .expect("Current frame always exists")
            .node_refs
            .get(&node_id)
            .cloned()
            .ok_or_else(|| RuntimeError::SubstateReadSubstateNotFound(substate_id.clone()))?;

        if matches!(substate_id, SubstateId::ComponentInfo(..))
            && matches!(node_pointer, RENodePointer::Store(..))
        {
            track
                .acquire_lock(substate_id.clone(), false, false)
                .expect("Should never fail");
        }

        // Read current value
        let current_value = {
            let mut node_ref = node_pointer.to_ref_mut(call_frames, track);
            node_ref.read_scrypto_value(&substate_id)?
        };

        // TODO: Remove, integrate with substate borrow mechanism
        if matches!(substate_id, SubstateId::ComponentInfo(..))
            && matches!(node_pointer, RENodePointer::Store(..))
        {
            track.release_lock(substate_id.clone(), false);
        }

        Ok((node_pointer.clone(), current_value))
    }

    fn new_uuid(id_allocator: &mut IdAllocator, transaction_hash: Hash) -> u128 {
        id_allocator.new_uuid(transaction_hash).unwrap()
    }

    fn new_node_id(
        id_allocator: &mut IdAllocator,
        transaction_hash: Hash,
        re_node: &HeapRENode,
    ) -> RENodeId {
        match re_node {
            HeapRENode::Bucket(..) => {
                let bucket_id = id_allocator.new_bucket_id().unwrap();
                RENodeId::Bucket(bucket_id)
            }
            HeapRENode::Proof(..) => {
                let proof_id = id_allocator.new_proof_id().unwrap();
                RENodeId::Proof(proof_id)
            }
            HeapRENode::Worktop(..) => RENodeId::Worktop,
            HeapRENode::Vault(..) => {
                let vault_id = id_allocator.new_vault_id(transaction_hash).unwrap();
                RENodeId::Vault(vault_id)
            }
            HeapRENode::KeyValueStore(..) => {
                let kv_store_id = id_allocator.new_kv_store_id(transaction_hash).unwrap();
                RENodeId::KeyValueStore(kv_store_id)
            }
            HeapRENode::Package(..) => {
                // Security Alert: ensure ID allocating will practically never fail
                let package_address = id_allocator.new_package_address(transaction_hash).unwrap();
                RENodeId::Package(package_address)
            }
            HeapRENode::Resource(..) => {
                let resource_address = id_allocator.new_resource_address(transaction_hash).unwrap();
                RENodeId::ResourceManager(resource_address)
            }
            HeapRENode::Component(ref component, ..) => {
                let component_address = id_allocator
                    .new_component_address(
                        transaction_hash,
                        &component.package_address(),
                        component.blueprint_name(),
                    )
                    .unwrap();
                RENodeId::Component(component_address)
            }
            HeapRENode::System(..) => {
                panic!("Should not get here.");
            }
        }
    }

    fn run(
        &mut self,
        execution_entity: ExecutionEntity,
        fn_ident: &str,
        input: ScryptoValue,
    ) -> Result<(ScryptoValue, HashMap<RENodeId, HeapRootRENode>), RuntimeError> {
        let output = {
            let rtn = match execution_entity {
                ExecutionEntity::Function(type_name) => match type_name {
                    TypeName::TransactionProcessor => TransactionProcessor::static_main(
                        fn_ident, input, self,
                    )
                    .map_err(|e| match e {
                        TransactionProcessorError::InvalidRequestData(_) => panic!("Illegal state"),
                        TransactionProcessorError::InvalidMethod => panic!("Illegal state"),
                        TransactionProcessorError::RuntimeError(e) => e,
                    }),
                    TypeName::Package => ValidatedPackage::static_main(fn_ident, input, self)
                        .map_err(RuntimeError::PackageError),
                    TypeName::ResourceManager => {
                        ResourceManager::static_main(fn_ident, input, self)
                            .map_err(RuntimeError::ResourceManagerError)
                    }
                    TypeName::Blueprint(package_address, blueprint_name) => {
                        let output = {
                            let package = self
                                .track
                                .read_substate(SubstateId::Package(package_address))
                                .package()
                                .clone(); // TODO: remove copy
                            let wasm_metering_params = self.fee_table.wasm_metering_params();
                            let instrumented_code = self
                                .wasm_instrumenter
                                .instrument(package.code(), &wasm_metering_params)
                                .to_vec(); // TODO: remove copy
                            let mut instance = self.wasm_engine.instantiate(&instrumented_code);
                            let blueprint_abi = package
                                .blueprint_abi(&blueprint_name)
                                .expect("Blueprint should exist");
                            let export_name = &blueprint_abi
                                .get_fn_abi(fn_ident)
                                .unwrap()
                                .export_name
                                .to_string();
                            let mut runtime: Box<dyn WasmRuntime> =
                                Box::new(RadixEngineWasmRuntime::new(
                                    ScryptoActor::blueprint(
                                        package_address,
                                        blueprint_name.clone(),
                                    ),
                                    self,
                                ));
                            instance
                                .invoke_export(&export_name, &input, &mut runtime)
                                .map_err(|e| match e {
                                    // Flatten error code for more readable transaction receipt
                                    InvokeError::RuntimeError(e) => e,
                                    e @ _ => RuntimeError::InvokeError(e.into()),
                                })?
                        };

                        let package = self
                            .track
                            .read_substate(SubstateId::Package(package_address))
                            .package();
                        let blueprint_abi = package
                            .blueprint_abi(&blueprint_name)
                            .expect("Blueprint should exist");
                        let fn_abi = blueprint_abi.get_fn_abi(fn_ident).unwrap();
                        if !fn_abi.output.matches(&output.dom) {
                            Err(RuntimeError::InvalidFnOutput {
                                fn_ident: fn_ident.to_string(),
                                output: output.dom,
                            })
                        } else {
                            Ok(output)
                        }
                    }
                },
                ExecutionEntity::Method(_, state) => match state {
                    ExecutionState::Consumed(node_id) => match node_id {
                        RENodeId::Bucket(..) => {
                            Bucket::consuming_main(node_id, fn_ident, input, self)
                                .map_err(RuntimeError::BucketError)
                        }
                        RENodeId::Proof(..) => Proof::main_consume(node_id, fn_ident, input, self)
                            .map_err(RuntimeError::ProofError),
                        _ => panic!("Unexpected"),
                    },
                    ExecutionState::RENodeRef(node_id) => match node_id {
                        RENodeId::Bucket(bucket_id) => {
                            Bucket::main(bucket_id, fn_ident, input, self)
                                .map_err(RuntimeError::BucketError)
                        }
                        RENodeId::Proof(proof_id) => Proof::main(proof_id, fn_ident, input, self)
                            .map_err(RuntimeError::ProofError),
                        RENodeId::Worktop => {
                            Worktop::main(fn_ident, input, self).map_err(RuntimeError::WorktopError)
                        }
                        RENodeId::Vault(vault_id) => Vault::main(vault_id, fn_ident, input, self)
                            .map_err(RuntimeError::VaultError),
                        RENodeId::Component(component_address) => {
                            Component::main(component_address, fn_ident, input, self)
                                .map_err(RuntimeError::ComponentError)
                        }
                        RENodeId::ResourceManager(resource_address) => {
                            ResourceManager::main(resource_address, fn_ident, input, self)
                                .map_err(RuntimeError::ResourceManagerError)
                        }
                        RENodeId::System => {
                            System::main(fn_ident, input, self).map_err(RuntimeError::SystemError)
                        }
                        _ => panic!("Unexpected"),
                    },
                    ExecutionState::Component(
                        package_address,
                        blueprint_name,
                        component_address,
                    ) => {
                        let output = {
                            let package = self
                                .track
                                .read_substate(SubstateId::Package(package_address))
                                .package()
                                .clone(); // TODO: remove copy
                            let wasm_metering_params = self.fee_table.wasm_metering_params();
                            let instrumented_code = self
                                .wasm_instrumenter
                                .instrument(package.code(), &wasm_metering_params)
                                .to_vec(); // TODO: remove copy
                            let mut instance = self.wasm_engine.instantiate(&instrumented_code);
                            let blueprint_abi = package
                                .blueprint_abi(&blueprint_name)
                                .expect("Blueprint should exist");
                            let export_name = &blueprint_abi
                                .get_fn_abi(fn_ident)
                                .unwrap()
                                .export_name
                                .to_string();
                            let mut runtime: Box<dyn WasmRuntime> =
                                Box::new(RadixEngineWasmRuntime::new(
                                    ScryptoActor::Component(
                                        component_address,
                                        package_address.clone(),
                                        blueprint_name.clone(),
                                    ),
                                    self,
                                ));
                            instance
                                .invoke_export(&export_name, &input, &mut runtime)
                                .map_err(|e| match e {
                                    // Flatten error code for more readable transaction receipt
                                    InvokeError::RuntimeError(e) => e,
                                    e @ _ => RuntimeError::InvokeError(e.into()),
                                })?
                        };

                        let package = self
                            .track
                            .read_substate(SubstateId::Package(package_address))
                            .package();
                        let blueprint_abi = package
                            .blueprint_abi(&blueprint_name)
                            .expect("Blueprint should exist");
                        let fn_abi = blueprint_abi.get_fn_abi(fn_ident).unwrap();
                        if !fn_abi.output.matches(&output.dom) {
                            Err(RuntimeError::InvalidFnOutput {
                                fn_ident: fn_ident.to_string(),
                                output: output.dom,
                            })
                        } else {
                            Ok(output)
                        }
                    }
                    ExecutionState::AuthZone(frame_id) => {
                        AuthZone::main(frame_id, fn_ident, input, self)
                            .map_err(RuntimeError::AuthZoneError)
                    }
                },
            }?;

            rtn
        };

        // Process return data
        Self::process_return_data(&output)?;

        // Take values to return
        let values_to_take = output.node_ids();
        let (received_values, mut missing) = Self::current_frame_mut(&mut self.call_frames)
            .take_available_values(values_to_take, false)?;
        let first_missing_value = missing.drain().nth(0);
        if let Some(missing_node) = first_missing_value {
            return Err(RuntimeError::RENodeNotFound(missing_node));
        }

        // Check we have valid references to pass back
        for refed_component_address in &output.refed_component_addresses {
            let node_id = RENodeId::Component(*refed_component_address);
            if let Some(RENodePointer::Store(..)) = Self::current_frame_mut(&mut self.call_frames)
                .node_refs
                .get(&node_id)
            {
                // Only allow passing back global references
            } else {
                return Err(RuntimeError::InvokeMethodInvalidReferencePass(node_id));
            }
        }

        // drop proofs and check resource leak
        Self::current_frame_mut(&mut self.call_frames)
            .auth_zone
            .clear();
        Self::current_frame_mut(&mut self.call_frames).drop_owned_values()?;

        Ok((output, received_values))
    }

    fn current_frame_mut(call_frames: &mut Vec<CallFrame>) -> &mut CallFrame {
        call_frames.last_mut().expect("Current frame always exists")
    }

    fn current_frame(call_frames: &Vec<CallFrame>) -> &CallFrame {
        call_frames.last().expect("Current frame always exists")
    }
}

impl<'g, 's, W, I, C> SystemApi<'s, W, I, C> for Kernel<'g, 's, W, I, C>
where
    W: WasmEngine<I>,
    I: WasmInstance,
    C: FeeReserve,
{
    fn invoke_function(
        &mut self,
        type_name: TypeName,
        fn_ident: String,
        input: ScryptoValue,
    ) -> Result<ScryptoValue, RuntimeError> {
        trace!(
            self,
            Level::Debug,
            "Invoking function: {:?} {:?}",
            type_name,
            &fn_ident
        );

        if self.call_frames.len() == self.max_depth {
            return Err(RuntimeError::MaxCallDepthLimitReached);
        }

        self.fee_reserve
            .consume(
                self.fee_table
                    .system_api_cost(SystemApiCostingEntry::InvokeFunction {
                        type_name: type_name.clone(),
                        input: &input,
                    }),
                "invoke_function",
            )
            .map_err(RuntimeError::CostingError)?;

        self.fee_reserve
            .consume(
                self.fee_table
                    .run_function_cost(&type_name, fn_ident.as_str(), &input),
                "run_function",
            )
            .map_err(RuntimeError::CostingError)?;

        // Prevent vaults/kvstores from being moved
        Self::process_call_data(&input)?;

        // Figure out what buckets and proofs to move from this process
        let values_to_take = input.node_ids();
        let (taken_values, mut missing) = Self::current_frame_mut(&mut self.call_frames)
            .take_available_values(values_to_take, false)?;
        let first_missing_value = missing.drain().nth(0);
        if let Some(missing_value) = first_missing_value {
            return Err(RuntimeError::RENodeNotFound(missing_value));
        }

        let mut next_owned_values = HashMap::new();

        // Internal state update to taken values
        for (id, mut value) in taken_values {
            match &mut value.root_mut() {
                HeapRENode::Proof(proof) => proof.change_to_restricted(),
                _ => {}
            }
            next_owned_values.insert(id, value);
        }

        let mut locked_values = HashSet::<SubstateId>::new();

        // No authorization but state load
        let actor = match &type_name {
            TypeName::Blueprint(package_address, blueprint_name) => {
                self.track
                    .acquire_lock(SubstateId::Package(package_address.clone()), false, false)
                    .map_err(|e| match e {
                        TrackError::NotFound => RuntimeError::PackageNotFound(*package_address),
                        TrackError::Reentrancy => {
                            panic!("Package reentrancy error should never occur.")
                        }
                        TrackError::StateTrackError(..) => panic!("Unexpected"),
                    })?;
                locked_values.insert(SubstateId::Package(package_address.clone()));
                let package = self
                    .track
                    .read_substate(SubstateId::Package(package_address.clone()))
                    .package();
                let abi = package.blueprint_abi(blueprint_name).ok_or(
                    RuntimeError::BlueprintNotFound(
                        package_address.clone(),
                        blueprint_name.clone(),
                    ),
                )?;
                let fn_abi = abi
                    .get_fn_abi(&fn_ident)
                    .ok_or(RuntimeError::MethodDoesNotExist(fn_ident.clone()))?;
                if !fn_abi.input.matches(&input.dom) {
                    return Err(RuntimeError::InvalidFnInput { fn_ident });
                }

                REActor::Scrypto(ScryptoActor::blueprint(
                    *package_address,
                    blueprint_name.clone(),
                ))
            }
            TypeName::Package | TypeName::ResourceManager | TypeName::TransactionProcessor => {
                REActor::Native
            }
        };

        // Move this into higher layer, e.g. transaction processor
        let mut next_frame_node_refs = HashMap::new();
        if self.call_frames.len() == 1 {
            let mut component_addresses = HashSet::new();

            // Collect component addresses
            for component_address in &input.refed_component_addresses {
                component_addresses.insert(*component_address);
            }
            let input: TransactionProcessorRunInput = scrypto_decode(&input.raw).unwrap();
            for instruction in &input.instructions {
                match instruction {
                    ExecutableInstruction::CallFunction { arg, .. }
                    | ExecutableInstruction::CallMethod { arg, .. } => {
                        let scrypto_value = ScryptoValue::from_slice(&arg).unwrap();
                        component_addresses.extend(scrypto_value.refed_component_addresses);
                    }
                    _ => {}
                }
            }

            // Make components visible
            for component_address in component_addresses {
                // TODO: Check if component exists
                let node_id = RENodeId::Component(component_address);
                next_frame_node_refs.insert(node_id, RENodePointer::Store(node_id));
            }
        } else {
            // Pass argument references
            for refed_component_address in &input.refed_component_addresses {
                let node_id = RENodeId::Component(refed_component_address.clone());
                if let Some(pointer) = Self::current_frame_mut(&mut self.call_frames)
                    .node_refs
                    .get(&node_id)
                {
                    let mut visible = HashSet::new();
                    visible.insert(SubstateId::ComponentInfo(*refed_component_address));
                    next_frame_node_refs.insert(node_id.clone(), pointer.clone());
                } else {
                    return Err(RuntimeError::InvokeMethodInvalidReferencePass(node_id));
                }
            }
        }

        // start a new frame and run
        let (output, received_values) = {
            let frame = CallFrame::new_child(
                Self::current_frame(&self.call_frames).depth + 1,
                actor,
                next_owned_values,
                next_frame_node_refs,
                self,
            );
            self.call_frames.push(frame);
            self.run(ExecutionEntity::Function(type_name), &fn_ident, input)?
        };

        // Remove the last after clean-up
        self.call_frames.pop();

        // Release locked addresses
        for l in locked_values {
            // TODO: refactor after introducing `Lock` representation.
            self.track.release_lock(l.clone(), false);
        }

        // move buckets and proofs to this process.
        for (id, value) in received_values {
            trace!(self, Level::Debug, "Received value: {:?}", value);
            Self::current_frame_mut(&mut self.call_frames)
                .owned_heap_nodes
                .insert(id, value);
        }

        // Accept component references
        for refed_component_address in &output.refed_component_addresses {
            let node_id = RENodeId::Component(*refed_component_address);
            let mut visible = HashSet::new();
            visible.insert(SubstateId::ComponentInfo(*refed_component_address));
            Self::current_frame_mut(&mut self.call_frames)
                .node_refs
                .insert(node_id, RENodePointer::Store(node_id));
        }

        trace!(self, Level::Debug, "Invoking finished!");
        Ok(output)
    }

    fn invoke_method(
        &mut self,
        receiver: Receiver,
        fn_ident: String,
        input: ScryptoValue,
    ) -> Result<ScryptoValue, RuntimeError> {
        trace!(
            self,
            Level::Debug,
            "Invoking method: {:?} {:?}",
            receiver,
            &fn_ident
        );

        if Self::current_frame(&self.call_frames).depth == self.max_depth {
            return Err(RuntimeError::MaxCallDepthLimitReached);
        }

        self.fee_reserve
            .consume(
                self.fee_table
                    .system_api_cost(SystemApiCostingEntry::InvokeMethod {
                        receiver: receiver.clone(),
                        input: &input,
                    }),
                "invoke_method",
            )
            .map_err(RuntimeError::CostingError)?;

        self.fee_reserve
            .consume(
                self.fee_table
                    .run_method_cost(&receiver, fn_ident.as_str(), &input),
                "run_method",
            )
            .map_err(RuntimeError::CostingError)?;

        // Prevent vaults/kvstores from being moved
        Self::process_call_data(&input)?;

        // Figure out what buckets and proofs to move from this process
        let values_to_take = input.node_ids();
        let (taken_values, mut missing) = Self::current_frame_mut(&mut self.call_frames)
            .take_available_values(values_to_take, false)?;
        let first_missing_value = missing.drain().nth(0);
        if let Some(missing_value) = first_missing_value {
            return Err(RuntimeError::RENodeNotFound(missing_value));
        }

        let mut next_owned_values = HashMap::new();

        // Internal state update to taken values
        for (id, mut value) in taken_values {
            match &mut value.root_mut() {
                HeapRENode::Proof(proof) => proof.change_to_restricted(),
                _ => {}
            }
            next_owned_values.insert(id, value);
        }

        let mut locked_values = HashSet::new();
        let mut next_frame_node_refs = HashMap::new();

        // Authorization and state load
        let (actor, execution_state) = match &receiver {
            Receiver::Consumed(node_id) => {
                let heap_node = Self::current_frame_mut(&mut self.call_frames)
                    .owned_heap_nodes
                    .remove(node_id)
                    .ok_or(RuntimeError::RENodeNotFound(*node_id))?;

                // Lock Additional Substates
                match heap_node.root() {
                    HeapRENode::Bucket(bucket) => {
                        let resource_address = bucket.resource_address();
                        self.track
                            .acquire_lock(
                                SubstateId::ResourceManager(resource_address),
                                true,
                                false,
                            )
                            .expect("Should not fail.");
                        locked_values
                            .insert((SubstateId::ResourceManager(resource_address.clone()), false));
                        let node_id = RENodeId::ResourceManager(resource_address);
                        next_frame_node_refs.insert(node_id, RENodePointer::Store(node_id));
                    }
                    _ => {}
                }

                AuthModule::consumed_auth(
                    &fn_ident,
                    heap_node.root(),
                    &mut self.call_frames,
                    &mut self.track,
                )?;
                next_owned_values.insert(*node_id, heap_node);

                Ok((REActor::Native, ExecutionState::Consumed(*node_id)))
            }
            Receiver::NativeRENodeRef(node_id) => {
                let native_substate_id = match node_id {
                    RENodeId::Bucket(bucket_id) => SubstateId::Bucket(*bucket_id),
                    RENodeId::Proof(proof_id) => SubstateId::Proof(*proof_id),
                    RENodeId::ResourceManager(resource_address) => {
                        SubstateId::ResourceManager(*resource_address)
                    }
                    RENodeId::System => SubstateId::System,
                    RENodeId::Worktop => SubstateId::Worktop,
                    RENodeId::Component(component_address) => {
                        SubstateId::ComponentInfo(*component_address)
                    }
                    RENodeId::Vault(vault_id) => SubstateId::Vault(*vault_id),
                    _ => return Err(RuntimeError::MethodDoesNotExist(fn_ident.clone())),
                };

                let node_pointer = if Self::current_frame(&self.call_frames)
                    .owned_heap_nodes
                    .contains_key(&node_id)
                {
                    RENodePointer::Heap {
                        frame_id: Self::current_frame(&self.call_frames).depth,
                        root: node_id.clone(),
                        id: None,
                    }
                } else if let Some(pointer) = Self::current_frame(&self.call_frames)
                    .node_refs
                    .get(&node_id)
                {
                    pointer.clone()
                } else {
                    match node_id {
                        // Let these be globally accessible for now
                        // TODO: Remove when references cleaned up
                        RENodeId::ResourceManager(..) | RENodeId::System => {
                            RENodePointer::Store(*node_id)
                        }
                        _ => return Err(RuntimeError::InvokeMethodInvalidReceiver(*node_id)),
                    }
                };

                next_frame_node_refs.insert(node_id.clone(), node_pointer.clone());

                // Lock Substate
                let is_lock_fee = matches!(node_id, RENodeId::Vault(..))
                    && (&fn_ident == "lock_fee" || &fn_ident == "lock_contingent_fee");
                match node_pointer {
                    RENodePointer::Store(..) => {
                        self.track
                            .acquire_lock(native_substate_id.clone(), true, is_lock_fee)
                            .map_err(|e| match e {
                                TrackError::StateTrackError(
                                    StateTrackError::RENodeAlreadyTouched,
                                ) => RuntimeError::LockFeeError(LockFeeError::RENodeAlreadyTouched),
                                // TODO: Remove when references cleaned up
                                TrackError::NotFound => RuntimeError::RENodeNotFound(*node_id),
                                TrackError::Reentrancy => {
                                    RuntimeError::Reentrancy(native_substate_id.clone())
                                }
                            })?;
                        locked_values.insert((native_substate_id.clone(), is_lock_fee));
                    }
                    RENodePointer::Heap { .. } => {
                        if is_lock_fee {
                            return Err(RuntimeError::LockFeeError(LockFeeError::RENodeNotInTrack));
                        }
                    }
                }

                // Lock Additional Substates
                match node_id {
                    RENodeId::Component(..) => {
                        let package_address = {
                            let node_ref = node_pointer.to_ref(&self.call_frames, &mut self.track);
                            node_ref.component_info().package_address()
                        };
                        let package_substate_id = SubstateId::Package(package_address);
                        let package_node_id = RENodeId::Package(package_address);
                        self.track
                            .acquire_lock(package_substate_id.clone(), false, false)
                            .map_err(|e| match e {
                                TrackError::NotFound => panic!("Should exist"),
                                TrackError::Reentrancy => RuntimeError::PackageReentrancy,
                                TrackError::StateTrackError(..) => panic!("Unexpected"),
                            })?;
                        locked_values.insert((package_substate_id.clone(), false));
                        next_frame_node_refs
                            .insert(package_node_id, RENodePointer::Store(package_node_id));
                    }
                    RENodeId::Vault(..) => {
                        let resource_address = {
                            let node_ref = node_pointer.to_ref(&self.call_frames, &mut self.track);
                            node_ref.vault().resource_address()
                        };
                        let resource_substate_id = SubstateId::ResourceManager(resource_address);
                        let resource_node_id = RENodeId::ResourceManager(resource_address);
                        self.track
                            .acquire_lock(resource_substate_id.clone(), true, false)
                            .expect("Should never fail.");
                        locked_values.insert((resource_substate_id, false));
                        next_frame_node_refs
                            .insert(resource_node_id, RENodePointer::Store(resource_node_id));
                    }
                    _ => {}
                }

                // Lock Resource Managers in request
                // TODO: Remove when references cleaned up
                for resource_address in &input.resource_addresses {
                    let resource_substate_id =
                        SubstateId::ResourceManager(resource_address.clone());
                    let node_id = RENodeId::ResourceManager(resource_address.clone());
                    self.track
                        .acquire_lock(resource_substate_id.clone(), false, false)
                        .map_err(|e| match e {
                            TrackError::NotFound => RuntimeError::RENodeNotFound(node_id),
                            TrackError::Reentrancy => {
                                RuntimeError::Reentrancy(resource_substate_id)
                            }
                            TrackError::StateTrackError(..) => panic!("Unexpected"),
                        })?;

                    locked_values
                        .insert((SubstateId::ResourceManager(resource_address.clone()), false));
                    next_frame_node_refs.insert(node_id, RENodePointer::Store(node_id));
                }

                // Check method authorization
                AuthModule::ref_auth(
                    &fn_ident,
                    &input,
                    native_substate_id.clone(),
                    node_pointer.clone(),
                    &mut self.call_frames,
                    &mut self.track,
                )?;

                Ok((REActor::Native, ExecutionState::RENodeRef(*node_id)))
            }
            Receiver::AuthZoneRef => {
                for resource_address in &input.resource_addresses {
                    self.track
                        .acquire_lock(
                            SubstateId::ResourceManager(resource_address.clone()),
                            false,
                            false,
                        )
                        .map_err(|e| match e {
                            TrackError::NotFound => {
                                RuntimeError::ResourceManagerNotFound(resource_address.clone())
                            }
                            TrackError::Reentrancy => {
                                panic!("Package reentrancy error should never occur.")
                            }
                            TrackError::StateTrackError(..) => panic!("Unexpected"),
                        })?;
                    locked_values
                        .insert((SubstateId::ResourceManager(resource_address.clone()), false));
                    let node_id = RENodeId::ResourceManager(resource_address.clone());
                    next_frame_node_refs.insert(node_id, RENodePointer::Store(node_id));
                }
                Ok((
                    REActor::Native,
                    ExecutionState::AuthZone(self.call_frames.len() - 1),
                ))
            }
            Receiver::Component(component_address) => {
                let component_address = component_address.clone();

                // Find value
                let node_id = RENodeId::Component(component_address);
                let node_pointer = if Self::current_frame(&self.call_frames)
                    .owned_heap_nodes
                    .contains_key(&node_id)
                {
                    RENodePointer::Heap {
                        frame_id: Self::current_frame(&self.call_frames).depth,
                        root: node_id.clone(),
                        id: None,
                    }
                } else if let Some(pointer) = Self::current_frame(&self.call_frames)
                    .node_refs
                    .get(&node_id)
                {
                    pointer.clone()
                } else {
                    return Err(RuntimeError::InvokeMethodInvalidReceiver(node_id));
                };

                // Lock values and setup next frame
                match node_pointer {
                    RENodePointer::Store(..) => {
                        let substate_id = SubstateId::ComponentState(component_address);
                        self.track
                            .acquire_lock(substate_id.clone(), true, false)
                            .map_err(|e| match e {
                                TrackError::NotFound => {
                                    RuntimeError::ComponentNotFound(component_address)
                                }
                                TrackError::Reentrancy => {
                                    RuntimeError::ComponentReentrancy(component_address)
                                }
                                TrackError::StateTrackError(..) => {
                                    panic!("Unexpected")
                                }
                            })?;
                        locked_values.insert((substate_id.clone(), false));
                    }
                    _ => {}
                };

                match node_pointer {
                    RENodePointer::Store(..) => {
                        self.track
                            .acquire_lock(
                                SubstateId::ComponentInfo(component_address),
                                false,
                                false,
                            )
                            .expect("Component Info should not be locked for long periods of time");
                    }
                    _ => {}
                }

                let scrypto_actor = {
                    let node_ref = node_pointer.to_ref(&self.call_frames, &mut self.track);
                    let component = node_ref.component_info();
                    ScryptoActor::component(
                        component_address,
                        component.package_address(),
                        component.blueprint_name().to_string(),
                    )
                };

                // Lock additional substates
                let package_substate_id =
                    SubstateId::Package(scrypto_actor.package_address().clone());
                self.track
                    .acquire_lock(package_substate_id.clone(), false, false)
                    .expect("Should never fail");
                locked_values.insert((package_substate_id.clone(), false));

                // Check Method Authorization
                AuthModule::ref_auth(
                    &fn_ident,
                    &input,
                    SubstateId::ComponentState(component_address),
                    node_pointer.clone(),
                    &mut self.call_frames,
                    &mut self.track,
                )?;

                match node_pointer {
                    RENodePointer::Store(..) => {
                        self.track
                            .release_lock(SubstateId::ComponentInfo(component_address), false);
                    }
                    _ => {}
                }

                next_frame_node_refs.insert(node_id, node_pointer);

                let execution_state = ExecutionState::Component(
                    scrypto_actor.package_address().clone(),
                    scrypto_actor.blueprint_name().clone(),
                    component_address,
                );
                Ok((REActor::Scrypto(scrypto_actor), execution_state))
            }
        }?;

        // Pass argument references
        for refed_component_address in &input.refed_component_addresses {
            let node_id = RENodeId::Component(refed_component_address.clone());
            if let Some(pointer) = Self::current_frame(&self.call_frames)
                .node_refs
                .get(&node_id)
            {
                let mut visible = HashSet::new();
                visible.insert(SubstateId::ComponentInfo(*refed_component_address));
                next_frame_node_refs.insert(node_id.clone(), pointer.clone());
            } else {
                return Err(RuntimeError::InvokeMethodInvalidReferencePass(node_id));
            }
        }

        // start a new frame
        let (output, received_values) = {
            let frame = CallFrame::new_child(
                Self::current_frame(&self.call_frames).depth + 1,
                actor,
                next_owned_values,
                next_frame_node_refs,
                self,
            );
            self.call_frames.push(frame);
            self.run(
                ExecutionEntity::Method(receiver, execution_state),
                &fn_ident,
                input,
            )?
        };

        // Remove the last after clean-up
        self.call_frames.pop();

        // Release locked addresses
        for (substate_id, write_through) in locked_values {
            // TODO: refactor after introducing `Lock` representation.
            self.track.release_lock(substate_id.clone(), write_through);
        }

        // move buckets and proofs to this process.
        for (id, value) in received_values {
            trace!(self, Level::Debug, "Received value: {:?}", value);
            Self::current_frame_mut(&mut self.call_frames)
                .owned_heap_nodes
                .insert(id, value);
        }

        // Accept component references
        for refed_component_address in &output.refed_component_addresses {
            let node_id = RENodeId::Component(*refed_component_address);
            let mut visible = HashSet::new();
            visible.insert(SubstateId::ComponentInfo(*refed_component_address));
            Self::current_frame_mut(&mut self.call_frames)
                .node_refs
                .insert(node_id, RENodePointer::Store(node_id));
        }

        trace!(self, Level::Debug, "Invoking finished!");
        Ok(output)
    }

    fn borrow_node(&mut self, node_id: &RENodeId) -> Result<RENodeRef<'_, 's>, FeeReserveError> {
        trace!(self, Level::Debug, "Borrowing value: {:?}", node_id);

        self.fee_reserve.consume(
            self.fee_table.system_api_cost({
                match node_id {
                    RENodeId::Bucket(_) => SystemApiCostingEntry::BorrowLocal,
                    RENodeId::Proof(_) => SystemApiCostingEntry::BorrowLocal,
                    RENodeId::Worktop => SystemApiCostingEntry::BorrowLocal,
                    RENodeId::Vault(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    RENodeId::Component(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    RENodeId::KeyValueStore(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    RENodeId::ResourceManager(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    RENodeId::Package(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    RENodeId::System => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                }
            }),
            "borrow",
        )?;

        let node_pointer = Self::current_frame(&self.call_frames)
            .node_refs
            .get(node_id)
            .expect(&format!("{:?} is unknown.", node_id));

        Ok(node_pointer.to_ref(&self.call_frames, &self.track))
    }

    fn substate_borrow_mut(
        &mut self,
        substate_id: &SubstateId,
    ) -> Result<NativeSubstateRef, FeeReserveError> {
        trace!(
            self,
            Level::Debug,
            "Borrowing substate (mut): {:?}",
            substate_id
        );

        // Costing
        self.fee_reserve.consume(
            self.fee_table.system_api_cost({
                match substate_id {
                    SubstateId::Bucket(_) => SystemApiCostingEntry::BorrowLocal,
                    SubstateId::Proof(_) => SystemApiCostingEntry::BorrowLocal,
                    SubstateId::Worktop => SystemApiCostingEntry::BorrowLocal,
                    SubstateId::Vault(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::ComponentState(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::ComponentInfo(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::KeyValueStoreSpace(_) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::KeyValueStoreEntry(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::ResourceManager(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::NonFungibleSpace(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::NonFungible(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::Package(..) => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                    SubstateId::System => SystemApiCostingEntry::BorrowGlobal {
                        // TODO: figure out loaded state and size
                        loaded: false,
                        size: 0,
                    },
                }
            }),
            "borrow",
        )?;

        // Authorization
        if !Self::current_frame(&self.call_frames)
            .actor
            .is_substate_readable(substate_id)
        {
            panic!("Trying to read value which is not visible.")
        }

        let node_id = SubstateProperties::get_node_id(substate_id);

        let node_pointer = Self::current_frame(&self.call_frames)
            .node_refs
            .get(&node_id)
            .cloned()
            .expect(&format!("Node should exist {:?}", node_id));

        Ok(node_pointer.borrow_native_ref(
            substate_id.clone(),
            &mut self.call_frames,
            &mut self.track,
        ))
    }

    fn substate_return_mut(&mut self, val_ref: NativeSubstateRef) -> Result<(), FeeReserveError> {
        trace!(self, Level::Debug, "Returning value");

        self.fee_reserve.consume(
            self.fee_table.system_api_cost({
                match &val_ref {
                    NativeSubstateRef::Stack(..) => SystemApiCostingEntry::ReturnLocal,
                    NativeSubstateRef::Track(substate_id, _) => match substate_id {
                        SubstateId::Vault(_) => SystemApiCostingEntry::ReturnGlobal { size: 0 },
                        SubstateId::KeyValueStoreSpace(_) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::KeyValueStoreEntry(_, _) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::ResourceManager(_) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::Package(_) => SystemApiCostingEntry::ReturnGlobal { size: 0 },
                        SubstateId::NonFungibleSpace(_) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::NonFungible(_, _) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::ComponentInfo(..) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::ComponentState(_) => {
                            SystemApiCostingEntry::ReturnGlobal { size: 0 }
                        }
                        SubstateId::System => SystemApiCostingEntry::ReturnGlobal { size: 0 },
                        SubstateId::Bucket(..) => SystemApiCostingEntry::ReturnGlobal { size: 0 },
                        SubstateId::Proof(..) => SystemApiCostingEntry::ReturnGlobal { size: 0 },
                        SubstateId::Worktop => SystemApiCostingEntry::ReturnGlobal { size: 0 },
                    },
                }
            }),
            "return",
        )?;

        val_ref.return_to_location(&mut self.call_frames, &mut self.track);
        Ok(())
    }

    fn node_drop(&mut self, node_id: &RENodeId) -> Result<HeapRootRENode, FeeReserveError> {
        trace!(self, Level::Debug, "Dropping value: {:?}", node_id);

        // TODO: costing

        // TODO: Authorization

        Ok(Self::current_frame_mut(&mut self.call_frames)
            .owned_heap_nodes
            .remove(&node_id)
            .unwrap())
    }

    fn node_create(&mut self, re_node: HeapRENode) -> Result<RENodeId, RuntimeError> {
        trace!(self, Level::Debug, "Creating value");

        // Costing
        self.fee_reserve
            .consume(
                self.fee_table
                    .system_api_cost(SystemApiCostingEntry::Create {
                        size: 0, // TODO: get size of the value
                    }),
                "create",
            )
            .map_err(RuntimeError::CostingError)?;

        // TODO: Authorization

        // Take any required child nodes
        let children = re_node.get_child_nodes()?;
        let (taken_root_nodes, mut missing) =
            Self::current_frame_mut(&mut self.call_frames).take_available_values(children, true)?;
        let first_missing_node = missing.drain().nth(0);
        if let Some(missing_node) = first_missing_node {
            return Err(RuntimeError::RENodeCreateNodeNotFound(missing_node));
        }
        let mut child_nodes = HashMap::new();
        for (id, taken_root_node) in taken_root_nodes {
            child_nodes.extend(taken_root_node.to_nodes(id));
        }

        // Insert node into heap
        let node_id = Self::new_node_id(&mut self.id_allocator, self.transaction_hash, &re_node);
        let heap_root_node = HeapRootRENode {
            root: re_node,
            child_nodes,
        };
        Self::current_frame_mut(&mut self.call_frames)
            .owned_heap_nodes
            .insert(node_id, heap_root_node);

        // TODO: Clean the following up
        match node_id {
            RENodeId::KeyValueStore(..) | RENodeId::ResourceManager(..) => {
                let frame = self
                    .call_frames
                    .last_mut()
                    .expect("Current frame always exists");
                frame.node_refs.insert(
                    node_id.clone(),
                    RENodePointer::Heap {
                        frame_id: frame.depth,
                        root: node_id.clone(),
                        id: None,
                    },
                );
            }
            RENodeId::Component(component_address) => {
                let mut visible = HashSet::new();
                visible.insert(SubstateId::ComponentInfo(component_address));

                let frame = self
                    .call_frames
                    .last_mut()
                    .expect("Current frame always exists");
                frame.node_refs.insert(
                    node_id.clone(),
                    RENodePointer::Heap {
                        frame_id: frame.depth,
                        root: node_id.clone(),
                        id: None,
                    },
                );
            }
            _ => {}
        }

        Ok(node_id)
    }

    fn node_globalize(&mut self, node_id: RENodeId) -> Result<(), RuntimeError> {
        trace!(self, Level::Debug, "Globalizing value: {:?}", node_id);

        // Costing
        self.fee_reserve
            .consume(
                self.fee_table
                    .system_api_cost(SystemApiCostingEntry::Globalize {
                        size: 0, // TODO: get size of the value
                    }),
                "globalize",
            )
            .map_err(RuntimeError::CostingError)?;

        if !RENodeProperties::can_globalize(node_id) {
            return Err(RuntimeError::RENodeGlobalizeTypeNotAllowed(node_id));
        }

        // TODO: Authorization

        let mut nodes_to_take = HashSet::new();
        nodes_to_take.insert(node_id);
        let (taken_nodes, missing_nodes) = Self::current_frame_mut(&mut self.call_frames)
            .take_available_values(nodes_to_take, false)?;
        assert!(missing_nodes.is_empty());
        assert!(taken_nodes.len() == 1);
        let root_node = taken_nodes.into_values().nth(0).unwrap();

        let (substates, maybe_non_fungibles) = match root_node.root {
            HeapRENode::Component(component, component_state) => {
                let mut substates = HashMap::new();
                let component_address = node_id.into();
                substates.insert(
                    SubstateId::ComponentInfo(component_address),
                    Substate::Component(component),
                );
                substates.insert(
                    SubstateId::ComponentState(component_address),
                    Substate::ComponentState(component_state),
                );
                let mut visible_substates = HashSet::new();
                visible_substates.insert(SubstateId::ComponentInfo(component_address));
                (substates, None)
            }
            HeapRENode::Package(package) => {
                let mut substates = HashMap::new();
                let package_address = node_id.into();
                substates.insert(
                    SubstateId::Package(package_address),
                    Substate::Package(package),
                );
                (substates, None)
            }
            HeapRENode::Resource(resource_manager, non_fungibles) => {
                let mut substates = HashMap::new();
                let resource_address: ResourceAddress = node_id.into();
                substates.insert(
                    SubstateId::ResourceManager(resource_address),
                    Substate::Resource(resource_manager),
                );
                (substates, non_fungibles)
            }
            _ => panic!("Not expected"),
        };

        for (substate_id, substate) in substates {
            self.track
                .create_uuid_substate(substate_id.clone(), substate);
        }

        let mut to_store_values = HashMap::new();
        for (id, value) in root_node.child_nodes.into_iter() {
            to_store_values.insert(id, value);
        }
        insert_non_root_nodes(self.track, to_store_values);

        if let Some(non_fungibles) = maybe_non_fungibles {
            let resource_address: ResourceAddress = node_id.into();
            let parent_address = SubstateId::NonFungibleSpace(resource_address.clone());
            for (id, non_fungible) in non_fungibles {
                self.track.set_key_value(
                    parent_address.clone(),
                    id.to_vec(),
                    Substate::NonFungible(NonFungibleWrapper(Some(non_fungible))),
                );
            }
        }

        Self::current_frame_mut(&mut self.call_frames)
            .node_refs
            .insert(node_id, RENodePointer::Store(node_id));

        Ok(())
    }

    fn substate_read(&mut self, substate_id: SubstateId) -> Result<ScryptoValue, RuntimeError> {
        trace!(self, Level::Debug, "Reading value data: {:?}", substate_id);

        // Costing
        self.fee_reserve
            .consume(
                self.fee_table.system_api_cost(SystemApiCostingEntry::Read {
                    size: 0, // TODO: get size of the value
                }),
                "read",
            )
            .map_err(RuntimeError::CostingError)?;

        // Authorization
        if !Self::current_frame(&self.call_frames)
            .actor
            .is_substate_readable(&substate_id)
        {
            return Err(RuntimeError::SubstateReadNotReadable(
                Self::current_frame(&self.call_frames).actor.clone(),
                substate_id.clone(),
            ));
        }

        let (parent_pointer, current_value) =
            Self::read_value_internal(&mut self.call_frames, self.track, &substate_id)?;
        let cur_children = current_value.node_ids();
        for child_id in cur_children {
            let child_pointer = parent_pointer.child(child_id);
            Self::current_frame_mut(&mut self.call_frames)
                .node_refs
                .insert(child_id, child_pointer);
        }
        Ok(current_value)
    }

    fn substate_take(&mut self, substate_id: SubstateId) -> Result<ScryptoValue, RuntimeError> {
        trace!(self, Level::Debug, "Removing value data: {:?}", substate_id);

        // TODO: Costing

        // Authorization
        if !Self::current_frame(&self.call_frames)
            .actor
            .is_substate_writeable(&substate_id)
        {
            return Err(RuntimeError::SubstateWriteNotWriteable(
                Self::current_frame(&self.call_frames).actor.clone(),
                substate_id,
            ));
        }

        let (pointer, current_value) =
            Self::read_value_internal(&mut self.call_frames, self.track, &substate_id)?;
        let cur_children = current_value.node_ids();
        if !cur_children.is_empty() {
            return Err(RuntimeError::ValueNotAllowed);
        }

        // Write values
        let mut node_ref = pointer.to_ref_mut(&mut self.call_frames, &mut self.track);
        node_ref.replace_value_with_default(&substate_id);

        Ok(current_value)
    }

    fn substate_write(
        &mut self,
        substate_id: SubstateId,
        value: ScryptoValue,
    ) -> Result<(), RuntimeError> {
        trace!(self, Level::Debug, "Writing value data: {:?}", substate_id);

        // Costing
        self.fee_reserve
            .consume(
                self.fee_table
                    .system_api_cost(SystemApiCostingEntry::Write {
                        size: 0, // TODO: get size of the value
                    }),
                "write",
            )
            .map_err(RuntimeError::CostingError)?;

        // Authorization
        if !Self::current_frame(&self.call_frames)
            .actor
            .is_substate_writeable(&substate_id)
        {
            return Err(RuntimeError::SubstateWriteNotWriteable(
                Self::current_frame(&self.call_frames).actor.clone(),
                substate_id,
            ));
        }

        // If write, take values from current frame
        let (taken_nodes, missing_nodes) = {
            let node_ids = value.node_ids();
            if !node_ids.is_empty() {
                if !SubstateProperties::can_own_nodes(&substate_id) {
                    return Err(RuntimeError::ValueNotAllowed);
                }

                Self::current_frame_mut(&mut self.call_frames)
                    .take_available_values(node_ids, true)?
            } else {
                (HashMap::new(), HashSet::new())
            }
        };

        let (pointer, current_value) =
            Self::read_value_internal(&mut self.call_frames, self.track, &substate_id)?;
        let cur_children = current_value.node_ids();

        // Fulfill method
        verify_stored_value_update(&cur_children, &missing_nodes)?;

        // TODO: verify against some schema

        // Write values
        let mut node_ref = pointer.to_ref_mut(&mut self.call_frames, &mut self.track);
        node_ref.write_value(substate_id, value, taken_nodes);

        Ok(())
    }

    fn transaction_hash(&mut self) -> Result<Hash, FeeReserveError> {
        self.fee_reserve.consume(
            self.fee_table
                .system_api_cost(SystemApiCostingEntry::ReadTransactionHash),
            "read_transaction_hash",
        )?;
        Ok(self.transaction_hash)
    }

    fn generate_uuid(&mut self) -> Result<u128, FeeReserveError> {
        self.fee_reserve.consume(
            self.fee_table
                .system_api_cost(SystemApiCostingEntry::GenerateUuid),
            "generate_uuid",
        )?;
        Ok(Self::new_uuid(
            &mut self.id_allocator,
            self.transaction_hash,
        ))
    }

    fn emit_log(&mut self, level: Level, message: String) -> Result<(), FeeReserveError> {
        self.fee_reserve.consume(
            self.fee_table
                .system_api_cost(SystemApiCostingEntry::EmitLog {
                    size: message.len() as u32,
                }),
            "emit_log",
        )?;
        self.track.add_log(level, message);
        Ok(())
    }

    fn check_access_rule(
        &mut self,
        access_rule: scrypto::resource::AccessRule,
        proof_ids: Vec<ProofId>,
    ) -> Result<bool, RuntimeError> {
        // TODO: costing

        let proofs = proof_ids
            .iter()
            .map(|proof_id| {
                Self::current_frame(&self.call_frames)
                    .owned_heap_nodes
                    .get(&RENodeId::Proof(*proof_id))
                    .map(|p| match p.root() {
                        HeapRENode::Proof(proof) => proof.clone(),
                        _ => panic!("Expected proof"),
                    })
                    .ok_or(RuntimeError::ProofNotFound(proof_id.clone()))
            })
            .collect::<Result<Vec<Proof>, RuntimeError>>()?;
        let mut simulated_auth_zone = AuthZone::new_with_proofs(proofs);

        let method_authorization = convert(&Type::Unit, &Value::Unit, &access_rule);
        let is_authorized = method_authorization.check(&[&simulated_auth_zone]).is_ok();
        simulated_auth_zone.clear();

        Ok(is_authorized)
    }

    fn fee_reserve(&mut self) -> &mut C {
        self.fee_reserve
    }

    fn auth_zone(&mut self, frame_id: usize) -> &mut AuthZone {
        &mut self.call_frames.get_mut(frame_id).unwrap().auth_zone
    }
}