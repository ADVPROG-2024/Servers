use std::collections::HashMap;
use wg_2024::config::Client;
use wg_2024::network::SourceRoutingHeader;
use wg_2024::packet::{Ack, FloodRequest, FloodResponse, Nack};

struct Message<M> {
    /// Used and modified by drones to route the packet.
    source_routing_header: SourceRoutingHeader,
    fragment_header: FragmentHeader,
    message_header: MessageHeader,
    m: M,
}
struct MessageHeader {
    /// ID of client or server
    source_id: Option<u64>,
}
// The serialized and possibly fragmented message sent by 
// either the client or server identified by source_id.
enum MessageType {
    webserver_message(WebserverMessage),
    chat_message_request(ChatMessageRequest),
    chat_message_response: ,
}

enum ServerType {
    CommunicationServer,
    TextServer
}
enum WebserverMessage {
    ServerTypeRequest,
    ServerTypeResponse(ServerType),
    FilesListRequest,
    FilesListResponse {
        list_length: char,
        list_of_file_ids: [u64; ],
    },
    ErrorNoFilesResponse,
    FileRequest {
    },
    FileResponse {
        file_size: u64,
        file: String,
    },
    ErrorFileNotFoundResponse,
    MediaRequest {
        media_id: &'static str,
    },
    MediaResponse {
        media_id: u64,
        media_size: u64,
        media: std::fs::File,
    },
    ErrorNoMediaResponse,
    ErrorMediaNotFoundResponse,
}
enum ChatMessageRequest {
    ClientListRequest,
    MessageForRequest {
        client_id: u64,
        message_size: Box<char>,
        message: [char; ],
    },
    ClientListResponse {
    },
}

// fragment defined as part of a message.
pub struct Fragment {
    fragment_index: u64,
    total_n_fragments: u64,
    length: u8,
    // assembler will fragment/de-fragment data into bytes.
    data: [u8; 128] // usable for image with .into_bytes()
}

struct FragmentHeader {
    /// Identifies the session to which this fragment belongs.
    session_id: u64,
    /// Total number of fragments, must be equal or greater than
    total_n_fragments: u64,
    /// Index of the packet, from 0 up to total_n_fragments - 1
    fragment_index: String,
    next_fragment: NextFragment,
}
type NextFragment = Option<Box<FragmentHeader>;

struct CommunicationServer {
    server_type: ServerType,
    clients: HashMap<String, Client>,
}

impl CommunicationServer {
    fn new() -> CommunicationServer {
        CommunicationServer {
            server_type: ServerType::CommunicationServer,
            clients: HashMap::new(),
        }
    }

    fn register_client(&mut self, name: &str, client: Client) {
        self.clients.insert(name.to_string(), client);
    }


}

fn main() {}