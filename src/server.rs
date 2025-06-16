use std::fmt::Display;
use crossbeam_channel::Sender;
use dronegowski_utils::hosts::ServerCommand;
use wg_2024::network::{NodeId};
use wg_2024::packet::{NodeType, Packet};
use serde::de::DeserializeOwned;


pub trait DronegowskiServer {
    //fn new(id: NodeId, sim_controller_send: Sender<ServerEvent>, sim_controller_recv: Receiver<ServerCommand>, packet_recv: Receiver<Packet>, packet_send: HashMap<NodeId, Sender<Packet>>, server_type: ServerType) -> Self;
    fn network_discovery(&mut self);
    fn run(&mut self);
    fn handle_packet(&mut self, packet: Packet);
    fn handle_command(&mut self, command: ServerCommand);
    fn update_graph(&mut self, path_trace: Vec<(NodeId, NodeType)>);
    fn compute_best_path(&self, target_client: NodeId) -> Option<Vec<NodeId>>;
    fn reconstruct_message<T: DeserializeOwned>(&mut self, key: u64) -> Result<T, Box<dyn std::error::Error>>;
    fn add_neighbor(&mut self, node_id: NodeId, sender: Sender<Packet>);
    fn remove_neighbor(&mut self, node_id: NodeId);
    fn remove_from_topology(&mut self, node_id: NodeId);
}
