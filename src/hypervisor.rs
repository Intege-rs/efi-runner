use std::future::Future;
use std::time::Duration;
use hcs_rs::compute::defs::{HcsOperationHandle, HcsSystemHandle};
use hcs_rs::compute::errorcodes::ResultCode;
use hcs_rs::computecore::{close_operation, create_compute_system, create_operation, get_compute_system_properties, get_operation_result, start_compute_system};
use hcs_rs::HcsResult;
use hcs_rs::schema::{ComputeSystem, Version, VirtualMachine};
use hcs_rs::schema::virtual_machines::Devices;
use hcs_rs::schema::virtual_machines::resources::{Chipset, ComPort, SerialConsole, Uefi, UefiBootDevice, UefiBootEntry};
use hcs_rs::schema::virtual_machines::resources::compute::{Memory, Processor, Topology};
use hcs_rs::schema::virtual_machines::resources::storage::{Attachment, AttachmentType, Scsi, VirtualSmb, VirtualSmbShare, VirtualSmbShareOptions};
use serde_json::Value;
use crate::CLIArgs;

#[allow(unused)]
pub struct Hypervisor {
    name: String,
    unique_id: String,
    handle: HcsSystemHandle,
}

impl Hypervisor {
    pub async fn build(name: &str, cliargs: CLIArgs) -> HcsResult<Self> {
        let com1_pipe = format!("\\\\.\\pipe\\vm_{name}_com1");
        
        let folder = cliargs.efi_file.parent().unwrap();
        let efi_file_folder = folder.to_string_lossy().as_ref().to_string();
        let efi_file_name = cliargs.efi_file.file_name().unwrap()
            .to_string_lossy().to_string();
        
        
        let compute_config = serde_json::to_string(&ComputeSystem {
            owner: name.to_string(),
            schema_version: Version::schema_version_19h1(),
            virtual_machine: Some(VirtualMachine {
                stop_on_reset: true,
                chipset: Chipset {
                    uefi: Some(Uefi {
                        enable_debugger: false,
                        secure_boot_template_id: None,
                        boot_this: Some(UefiBootEntry {
                            device_type: UefiBootDevice::VmbFs,
                            device_path: efi_file_name, // cliargs.efi_file.to_string(),
                            disk_number: 0,
                            ..Default::default()
                        }),
                        console: SerialConsole::ComPort1,
                        stop_on_boot_failure: false,
                    }),
                    ..Default::default()
                },
                compute_topology: Topology {
                    memory: Memory {
                        size_in_mb: cliargs.memory as u64,
                        ..Default::default()
                    },
                    processor: Processor {
                        count: cliargs.cores as u32,
                        limit: None,
                        weight: None,
                        expose_virtualization_extensions: true,
                        ..Default::default()
                    },
                },
                devices: Devices {
                    com_ports: [(0u32, ComPort { named_pipe: com1_pipe.clone(), optimize_for_debugger: false }), ].into_iter().collect(),
                    scsi: cliargs.disks.iter().enumerate().map(|(index, disk)| {
                        (
                            format!("disk-{index}"),
                            Scsi {
                                attachments: [
                                    (0u32, Attachment {
                                        attachment_type: AttachmentType::VirtualDisk,
                                        path: disk.to_string_lossy().to_string(),
                                        ..Default::default()
                                    }),
                                ].into_iter().collect(),
                            }
                        )
                    }).collect(),
                    virtual_smb: Some(
                        VirtualSmb {
                            shares: vec![VirtualSmbShare {
                                name: "smb".to_string(),
                                path: efi_file_folder,
                                allowed_files: vec![],
                                options: VirtualSmbShareOptions {
                                    restrict_file_access    : false,
                                    single_file_mapping     : true,
                                    
                                    read_only               : true,
                                    pseudo_oplocks          : true,
                                    take_backup_privilege   : true,
                                    cache_io                : true,
                                    share_read              : true,
                                    ..Default::default()
                                },
                            }],
                            direct_file_mapping_in_mb: 128,
                        }
                    ),
                    ..Default::default()
                },
                ..Default::default()
            }),
            should_terminate_on_last_handle_closed: true,
            ..Default::default()
        }).unwrap();
        
        // construct the virtual machine
        let operation = async_operation()?;
        let handle = create_compute_system(name, compute_config.as_str(), operation.0, None)?;
        operation.1.await.map_err(|(code, response)| {
            if let Ok(value) = serde_json::from_str::<Value>(response.as_str()) {
                eprintln!("{}", serde_json::to_string_pretty(&value).unwrap())
            }
            code
        })?;
        
        // get the current runtime id
        let operation = async_operation()?;
        get_compute_system_properties(handle, operation.0, Some("{\"PropertyTypes\": [\"GuestConnection\"]}"))?;
        let response = operation.1.await.map_err(|t| t.0)?;
        let response = serde_json::from_str::<Value>(response.as_str()).unwrap();
        let unique_id = response["RuntimeId"].as_str().unwrap().to_string();

        // start the virtual machine
        let operation = async_operation()?;
        start_compute_system(handle, operation.0, None)?;
        operation.1.await.map_err(|(code, response)| {
            if let Ok(value) = serde_json::from_str::<Value>(response.as_str()) {
                eprintln!("{}", serde_json::to_string_pretty(&value).unwrap())
            }
            code
        })?;

        Self::proxy_serial(com1_pipe.as_str()).await.map_err(|e| {
            eprintln!("Failed to open terminal: {:?}", e);
            ResultCode::Unexpected
        })?;

        Ok(Self {
            name: name.to_string(),
            unique_id,
            handle,
        })
    }

    async fn proxy_serial(pipe: &str) -> std::io::Result<()> {
        use tokio::net::windows::named_pipe::ClientOptions;
        use tokio::io::{copy_bidirectional};

        let mut client = loop {
            match ClientOptions::new().open(pipe) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(231i32) => (),
                Err(e) => { return Err(e)?; }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let mut terminal = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
        tokio::spawn(async move {
            if let Err(e) = copy_bidirectional(&mut terminal, &mut client).await {
                println!("Terminal Error: {e}");
            }
        });
        Ok(())
    }
}


type OperationResult = Result<String, (ResultCode, String)>;

fn async_operation() -> HcsResult<(HcsOperationHandle, impl Future<Output=OperationResult>)> {
    use tokio::sync::oneshot;

    // noinspection RsAssertEqual
    // make sure our sender can be passed safely
    const _ENSURE_SIZE: () = assert!(std::mem::size_of::<usize>() == std::mem::size_of::<oneshot::Sender<OperationResult>>(), "Cannot convert Receiver to usize!");

    let (tx, rx) = oneshot::channel::<OperationResult>();
    let handle = create_operation(unsafe { std::mem::transmute(tx) }, Some(_handler))?;


    unsafe extern "system" fn _handler(operation: HcsOperationHandle, context: *mut winapi::ctypes::c_void) {
        let sender: oneshot::Sender<OperationResult> = std::mem::transmute(context);
        let (response, err) = get_operation_result(operation);
        let _ = close_operation(operation);
        let _ = sender.send(match err {
            Ok(_) => Ok(response),
            Err(code) => Err((code, response)),
        });
    }

    // Sender cannot be dropped (its transmuted), so just strip the outer result
    Ok((handle, async { unsafe { rx.await.unwrap_unchecked() } }))
}