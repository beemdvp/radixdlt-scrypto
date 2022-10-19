use crate::engine::{CallFrameError, HeapRENode, Track};
use crate::types::{HashMap, HashSet};
use scrypto::engine::types::{RENodeId, SubstateId};
use crate::fee::FeeReserve;
use crate::model::node_to_substates;

pub struct Heap {
    pub nodes: HashMap<RENodeId, HeapRENode>,
}

impl Heap {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub fn get_node_mut(
        &mut self,
        node_id: RENodeId,
    ) -> Result<&mut HeapRENode, CallFrameError> {
        self.nodes
            .get_mut(&node_id)
            .ok_or(CallFrameError::RENodeNotOwned(node_id))
    }

    pub fn get_node(&self, node_id: RENodeId) -> Result<&HeapRENode, CallFrameError> {
        self.nodes
            .get(&node_id)
            .ok_or(CallFrameError::RENodeNotOwned(node_id))
    }

    pub fn create_node(&mut self, node_id: RENodeId, node: HeapRENode) {
        self.nodes.insert(node_id, node);
    }

    pub fn add_child_nodes(
        &mut self,
        node_ids: HashSet<RENodeId>,
        to: RENodeId,
    ) -> Result<(), CallFrameError> {
        for node_id in &node_ids {
            // Sanity check
            if !self.nodes.contains_key(&node_id) {
                return Err(CallFrameError::RENodeNotOwned(*node_id));
            }
        }

        self.get_node_mut(to)?.child_nodes.extend(node_ids);

        /*
        for node_id in node_ids {
            if !self.nodes.contains_key(&node_id) {
                return Err(CallFrameError::RENodeNotOwned(node_id));
            }

            //let node = self.remove_node(node_id)?;
            nodes.extend(node.to_nodes(node_id));
        }
             */

        //self.get_node_mut(to)?.child_nodes.extend(nodes);

        Ok(())
    }

    pub fn move_nodes_to_store<R: FeeReserve>(&mut self, track: &mut Track<R>, nodes: HashSet<RENodeId>) -> Result<(), CallFrameError> {
        for node_id in nodes {
            self.move_node_to_store(track, node_id)?;
        }

        Ok(())
    }

    pub fn move_node_to_store<R: FeeReserve>(&mut self, track: &mut Track<R>, node_id: RENodeId) -> Result<(), CallFrameError> {
        let node = self.nodes.remove(&node_id).ok_or(CallFrameError::RENodeNotOwned(node_id))?;
        let substates = node_to_substates(node.root);
        for (offset, substate) in substates {
            track.insert_substate(SubstateId(node_id, offset), substate);
        }

        self.move_nodes_to_store(track, node.child_nodes)?;

        Ok(())
    }

    pub fn remove_node(&mut self, node_id: RENodeId) -> Result<HeapRENode, CallFrameError> {
        self.nodes
            .remove(&node_id)
            .ok_or(CallFrameError::RENodeNotOwned(node_id))
    }
}
