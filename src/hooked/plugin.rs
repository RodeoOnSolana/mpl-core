#[cfg(feature = "anchor")]
use anchor_lang::prelude::AnchorDeserialize as CrateDeserialize;
#[cfg(not(feature = "anchor"))]
use borsh::BorshDeserialize as CrateDeserialize;
use num_traits::FromPrimitive;
use solana_program::{account_info::AccountInfo, pubkey::Pubkey};

use crate::{
    accounts::{BaseAssetV1, BaseCollectionV1, PluginHeaderV1},
    errors::MplCoreError,
    types::{
        ExternalPluginAdapter, ExternalPluginAdapterKey, ExternalPluginAdapterType, LinkedDataKey,
        Plugin, PluginAuthority, PluginType, RegistryRecord,
    },
    AddBlockerPlugin, AppDataWithData, AttributesPlugin, AutographPlugin, BaseAuthority,
    BasePlugin, BubblegumV2Plugin, BurnDelegatePlugin, DataBlob, DataSectionWithData,
    EditionPlugin, ExternalPluginAdaptersList, ExternalRegistryRecordSafe, FreezeDelegatePlugin,
    FreezeExecutePlugin, ImmutableMetadataPlugin, LifecycleHookWithData, MasterEditionPlugin,
    PermanentBurnDelegatePlugin, PermanentFreezeDelegatePlugin, PermanentFreezeExecutePlugin,
    PermanentTransferDelegatePlugin, PluginRegistryV1Safe, PluginsList, RegistryRecordSafe,
    RoyaltiesPlugin, SolanaAccount, TransferDelegatePlugin, UpdateDelegatePlugin,
    VerifiedCreatorsPlugin,
};

/// Fetch the plugin from the registry.
pub fn fetch_plugin<T: DataBlob + SolanaAccount, U: CrateDeserialize>(
    account: &AccountInfo,
    plugin_type: PluginType,
) -> Result<(PluginAuthority, U, usize), std::io::Error> {
    let asset = T::load(account, 0)?;

    if asset.len() == account.data_len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            MplCoreError::PluginNotFound.to_string(),
        ));
    }

    let header = PluginHeaderV1::from_bytes(&(*account.data).borrow()[asset.len()..])?;
    let plugin_registry = PluginRegistryV1Safe::from_bytes(
        &(*account.data).borrow()[header.plugin_registry_offset as usize..],
    )?;

    // Find the plugin in the registry.
    let registry_record = plugin_registry
        .registry
        .iter()
        .find(|record| {
            if let Some(plugin) = PluginType::from_u8(record.plugin_type) {
                plugin == plugin_type
            } else {
                false
            }
        })
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::Other,
            MplCoreError::PluginNotFound.to_string(),
        ))?;

    // Deserialize the plugin.
    let plugin =
        Plugin::deserialize(&mut &(*account.data).borrow()[(registry_record.offset as usize)..])?;

    if PluginType::from(&plugin) != plugin_type {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            MplCoreError::PluginNotFound.to_string(),
        ));
    }

    let inner = U::deserialize(
        &mut &(*account.data).borrow()[registry_record.offset.checked_add(1).ok_or(
            std::io::Error::new(
                std::io::ErrorKind::Other,
                MplCoreError::NumericalOverflow.to_string(),
            ),
        )? as usize..],
    )?;

    // Return the plugin and its authority.
    Ok((
        registry_record.authority.clone(),
        inner,
        registry_record.offset as usize,
    ))
}

/// Fetch the plugin on an asset.
pub fn fetch_asset_plugin<U: CrateDeserialize>(
    account: &AccountInfo,
    plugin_type: PluginType,
) -> Result<(PluginAuthority, U, usize), std::io::Error> {
    fetch_plugin::<BaseAssetV1, U>(account, plugin_type)
}

/// Fetch the plugin on a collection.
pub fn fetch_collection_plugin<U: CrateDeserialize>(
    account: &AccountInfo,
    plugin_type: PluginType,
) -> Result<(PluginAuthority, U, usize), std::io::Error> {
    fetch_plugin::<BaseCollectionV1, U>(account, plugin_type)
}

/// Fetch the plugin registry, dropping any unknown plugins (i.e. `PluginType`s that are too new
///  for this client to know about).
pub fn fetch_plugins(account_data: &[u8]) -> Result<Vec<RegistryRecord>, std::io::Error> {
    let asset = BaseAssetV1::from_bytes(account_data)?;

    let header = PluginHeaderV1::from_bytes(&account_data[asset.len()..])?;
    let plugin_registry = PluginRegistryV1Safe::from_bytes(
        &account_data[(header.plugin_registry_offset as usize)..],
    )?;

    let filtered_plugin_registry = plugin_registry
        .registry
        .iter()
        .filter_map(|record| {
            PluginType::from_u8(record.plugin_type).map(|plugin_type| RegistryRecord {
                plugin_type,
                authority: record.authority.clone(),
                offset: record.offset,
            })
        })
        .collect();

    Ok(filtered_plugin_registry)
}

/// Fetch the external plugin adapter from the registry.
pub fn fetch_external_plugin_adapter<T: DataBlob + SolanaAccount, U: CrateDeserialize>(
    account: &AccountInfo,
    core: Option<&T>,
    plugin_key: &ExternalPluginAdapterKey,
) -> Result<(PluginAuthority, U, usize), std::io::Error> {
    let registry_record = fetch_external_registry_record(account, core, plugin_key)?;

    let inner = U::deserialize(
        &mut &(*account.data).borrow()[registry_record.offset.checked_add(1).ok_or(
            std::io::Error::new(
                std::io::ErrorKind::Other,
                MplCoreError::NumericalOverflow.to_string(),
            ),
        )? as usize..],
    )?;

    // Return the plugin and its authority.
    Ok((
        registry_record.authority.clone(),
        inner,
        registry_record.offset as usize,
    ))
}

/// Fetch the external plugin adapter from the registry.
pub fn fetch_wrapped_external_plugin_adapter<T: DataBlob + SolanaAccount>(
    account: &AccountInfo,
    core: Option<&T>,
    plugin_key: &ExternalPluginAdapterKey,
) -> Result<(ExternalRegistryRecordSafe, ExternalPluginAdapter), std::io::Error> {
    let registry_record = fetch_external_registry_record(account, core, plugin_key)?;

    // Deserialize the plugin.
    let plugin = ExternalPluginAdapter::deserialize(
        &mut &(*account.data).borrow()[registry_record.offset as usize..],
    )?;

    // Return the plugin and its authority.
    Ok((registry_record, plugin))
}

// Helper to unwrap optional data offset and data length.
fn unwrap_data_offset_and_data_len(
    data_offset: Option<u64>,
    data_len: Option<u64>,
) -> Result<(usize, usize), std::io::Error> {
    let data_offset = data_offset.ok_or(std::io::Error::new(
        std::io::ErrorKind::Other,
        MplCoreError::InvalidPlugin.to_string(),
    ))?;

    let data_len = data_len.ok_or(std::io::Error::new(
        std::io::ErrorKind::Other,
        MplCoreError::InvalidPlugin.to_string(),
    ))?;

    Ok((data_offset as usize, data_len as usize))
}

/// Fetch the external plugin adapter data offset and length.  These can be used to
/// directly slice the account data for use of the external plugin adapter data.
pub fn fetch_external_plugin_adapter_data_info<T: DataBlob + SolanaAccount>(
    account: &AccountInfo,
    core: Option<&T>,
    plugin_key: &ExternalPluginAdapterKey,
) -> Result<(usize, usize), std::io::Error> {
    let registry_record = fetch_external_registry_record(account, core, plugin_key)?;
    let (data_offset, data_len) =
        unwrap_data_offset_and_data_len(registry_record.data_offset, registry_record.data_len)?;

    // Return the data offset and length.
    Ok((data_offset, data_len))
}

// Internal helper to fetch just the external registry record for the external plugin key.
fn fetch_external_registry_record<T: DataBlob + SolanaAccount>(
    account: &AccountInfo,
    core: Option<&T>,
    plugin_key: &ExternalPluginAdapterKey,
) -> Result<ExternalRegistryRecordSafe, std::io::Error> {
    let size = match core {
        Some(core) => core.len(),
        None => {
            let asset = T::load(account, 0)?;

            if asset.len() == account.data_len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    MplCoreError::ExternalPluginAdapterNotFound.to_string(),
                ));
            }

            asset.len()
        }
    };

    let header = PluginHeaderV1::from_bytes(&(*account.data).borrow()[size..])?;
    let plugin_registry = PluginRegistryV1Safe::from_bytes(
        &(*account.data).borrow()[header.plugin_registry_offset as usize..],
    )?;

    // Find and return the registry record.
    let result = find_external_plugin_adapter(&plugin_registry, plugin_key, account)?;
    result.1.cloned().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            MplCoreError::ExternalPluginAdapterNotFound.to_string(),
        )
    })
}

/// List all plugins in an account, dropping any unknown plugins (i.e. `PluginType`s that are too
/// new for this client to know about). Note this also does not support external plugin adapters for now,
/// and will be updated when those are defined.
pub fn list_plugins(account_data: &[u8]) -> Result<Vec<PluginType>, std::io::Error> {
    let asset = BaseAssetV1::from_bytes(account_data)?;
    let header = PluginHeaderV1::from_bytes(&account_data[asset.len()..])?;
    let plugin_registry = PluginRegistryV1Safe::from_bytes(
        &account_data[(header.plugin_registry_offset as usize)..],
    )?;

    Ok(plugin_registry
        .registry
        .iter()
        .filter_map(|registry_record| PluginType::from_u8(registry_record.plugin_type))
        .collect())
}

// Convert a slice of `RegistryRecordSafe` into the `PluginsList` type, dropping any unknown
// plugins (i.e. `PluginType`s that are too new for this client to know about).
pub(crate) fn registry_records_to_plugin_list(
    registry_records: &[RegistryRecordSafe],
    account_data: &[u8],
) -> Result<PluginsList, std::io::Error> {

    let mut result = PluginsList::default();
    for record in registry_records.iter() {
        if PluginType::from_u8(record.plugin_type).is_some() {
            let authority: BaseAuthority = record.authority.clone().into();
            let base = BasePlugin {
                authority,
                offset: Some(record.offset),
            };
            let plugin = Plugin::deserialize(&mut &account_data[record.offset as usize..])?;

            match plugin {
                Plugin::Royalties(royalties) => {
                    result.royalties = Some(RoyaltiesPlugin { base, royalties });
                }
                Plugin::FreezeDelegate(freeze_delegate) => {
                    result.freeze_delegate = Some(FreezeDelegatePlugin {
                        base,
                        freeze_delegate,
                    });
                }
                Plugin::BurnDelegate(burn_delegate) => {
                    result.burn_delegate = Some(BurnDelegatePlugin {
                        base,
                        burn_delegate,
                    });
                }
                Plugin::TransferDelegate(transfer_delegate) => {
                    result.transfer_delegate = Some(TransferDelegatePlugin {
                        base,
                        transfer_delegate,
                    });
                }
                Plugin::UpdateDelegate(update_delegate) => {
                    result.update_delegate = Some(UpdateDelegatePlugin {
                        base,
                        update_delegate,
                    });
                }
                Plugin::PermanentFreezeDelegate(permanent_freeze_delegate) => {
                    result.permanent_freeze_delegate = Some(PermanentFreezeDelegatePlugin {
                        base,
                        permanent_freeze_delegate,
                    });
                }
                Plugin::Attributes(attributes) => {
                    result.attributes = Some(AttributesPlugin { base, attributes });
                }
                Plugin::PermanentTransferDelegate(permanent_transfer_delegate) => {
                    result.permanent_transfer_delegate = Some(PermanentTransferDelegatePlugin {
                        base,
                        permanent_transfer_delegate,
                    })
                }
                Plugin::PermanentBurnDelegate(permanent_burn_delegate) => {
                    result.permanent_burn_delegate = Some(PermanentBurnDelegatePlugin {
                        base,
                        permanent_burn_delegate,
                    })
                }
                Plugin::Edition(edition) => result.edition = Some(EditionPlugin { base, edition }),
                Plugin::MasterEdition(master_edition) => {
                    result.master_edition = Some(MasterEditionPlugin {
                        base,
                        master_edition,
                    })
                }
                Plugin::AddBlocker(add_blocker) => {
                    result.add_blocker = Some(AddBlockerPlugin { base, add_blocker })
                }
                Plugin::ImmutableMetadata(immutable_metadata) => {
                    result.immutable_metadata = Some(ImmutableMetadataPlugin {
                        base,
                        immutable_metadata,
                    })
                }
                Plugin::VerifiedCreators(verified_creators) => {
                    result.verified_creators = Some(VerifiedCreatorsPlugin {
                        base,
                        verified_creators,
                    })
                }
                Plugin::Autograph(autograph) => {
                    result.autograph = Some(AutographPlugin { base, autograph })
                }
                Plugin::BubblegumV2(bubblegum_v2) => {
                    result.bubblegum_v2 = Some(BubblegumV2Plugin { base, bubblegum_v2 })
                }
                Plugin::FreezeExecute(freeze_execute) => {
                    result.freeze_execute = Some(FreezeExecutePlugin {
                        base,
                        freeze_execute,
                    });
                }
                Plugin::PermanentFreezeExecute(permanent_freeze_execute) => {
                    result.permanent_freeze_execute = Some(PermanentFreezeExecutePlugin {
                        base,
                        permanent_freeze_execute,
                    });
                }
            }
        }
    }
    Ok(result)

}

// Convert a slice of `AdapterRegistryRecordSafe` into the `ExternalPluginAdaptersList` type, dropping any unknown
// plugins (i.e. `ExternalPluginAdapterType`s that are too new for this client to know about).
pub(crate) fn registry_records_to_external_plugin_adapter_list(
    registry_records: &[ExternalRegistryRecordSafe],
    account_data: &[u8],
) -> Result<ExternalPluginAdaptersList, std::io::Error> {
    let result = registry_records.iter().try_fold(
        ExternalPluginAdaptersList::default(),
        |mut acc, record| {
            if ExternalPluginAdapterType::from_u8(record.plugin_type).is_some() {
                let plugin = ExternalPluginAdapter::deserialize(
                    &mut &account_data[record.offset as usize..],
                )?;

                match plugin {
                    ExternalPluginAdapter::LifecycleHook(lifecycle_hook) => {
                        let (data_offset, data_len) =
                            unwrap_data_offset_and_data_len(record.data_offset, record.data_len)?;

                        acc.lifecycle_hooks.push(LifecycleHookWithData {
                            base: lifecycle_hook,
                            data_offset,
                            data_len,
                        })
                    }
                    ExternalPluginAdapter::LinkedLifecycleHook(lifecycle_hook) => {
                        acc.linked_lifecycle_hooks.push(lifecycle_hook)
                    }
                    ExternalPluginAdapter::Oracle(oracle) => acc.oracles.push(oracle),
                    ExternalPluginAdapter::AppData(app_data) => {
                        let (data_offset, data_len) =
                            unwrap_data_offset_and_data_len(record.data_offset, record.data_len)?;

                        acc.app_data.push(AppDataWithData {
                            base: app_data,
                            data_offset,
                            data_len,
                        })
                    }
                    ExternalPluginAdapter::LinkedAppData(app_data) => {
                        acc.linked_app_data.push(app_data)
                    }
                    ExternalPluginAdapter::DataSection(data_section) => {
                        let (data_offset, data_len) =
                            unwrap_data_offset_and_data_len(record.data_offset, record.data_len)?;

                        acc.data_sections.push(DataSectionWithData {
                            base: data_section,
                            data_offset,
                            data_len,
                        })
                    }
                    ExternalPluginAdapter::AgentIdentity(agent_identity) => {
                        acc.agent_identities.push(agent_identity)
                    }
                }
            }
            Ok(acc)
        },
    );

    result
}

pub(crate) fn find_external_plugin_adapter<'b>(
    plugin_registry: &'b PluginRegistryV1Safe,
    plugin_key: &ExternalPluginAdapterKey,
    account: &AccountInfo<'_>,
) -> Result<(Option<usize>, Option<&'b ExternalRegistryRecordSafe>), std::io::Error> {
    let mut result = (None, None);
    for (i, record) in plugin_registry.external_registry.iter().enumerate() {
        if record.plugin_type == ExternalPluginAdapterType::from(plugin_key) as u8
            && (match plugin_key {
                ExternalPluginAdapterKey::LifecycleHook(address)
                | ExternalPluginAdapterKey::Oracle(address)
                | ExternalPluginAdapterKey::LinkedLifecycleHook(address) => {
                    let pubkey_offset = record.offset.checked_add(1).ok_or(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        MplCoreError::NumericalOverflow,
                    ))?;
                    address
                        == &match Pubkey::deserialize(
                            &mut &account.data.borrow()[pubkey_offset as usize..],
                        ) {
                            Ok(address) => address,
                            Err(_) => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    Box::<dyn std::error::Error + Send + Sync>::from(
                                        MplCoreError::DeserializationError,
                                    ),
                                ))
                            }
                        }
                }
                ExternalPluginAdapterKey::AppData(authority) => {
                    let authority_offset =
                        record.offset.checked_add(1).ok_or(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            MplCoreError::NumericalOverflow,
                        ))?;
                    authority
                        == &match PluginAuthority::deserialize(
                            &mut &account.data.borrow()[authority_offset as usize..],
                        ) {
                            Ok(authority) => authority,
                            Err(_) => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    Box::<dyn std::error::Error + Send + Sync>::from(
                                        MplCoreError::DeserializationError,
                                    ),
                                ))
                            }
                        }
                }
                ExternalPluginAdapterKey::LinkedAppData(authority) => {
                    let authority_offset =
                        record.offset.checked_add(1).ok_or(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            MplCoreError::NumericalOverflow,
                        ))?;
                    authority
                        == &match PluginAuthority::deserialize(
                            &mut &account.data.borrow()[authority_offset as usize..],
                        ) {
                            Ok(authority) => authority,
                            Err(_) => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    Box::<dyn std::error::Error + Send + Sync>::from(
                                        MplCoreError::DeserializationError,
                                    ),
                                ))
                            }
                        }
                }
                ExternalPluginAdapterKey::DataSection(linked_data_key) => {
                    let linked_data_key_offset =
                        record.offset.checked_add(1).ok_or(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            MplCoreError::NumericalOverflow,
                        ))?;
                    linked_data_key
                        == &match LinkedDataKey::deserialize(
                            &mut &account.data.borrow()[linked_data_key_offset as usize..],
                        ) {
                            Ok(linked_data_key) => linked_data_key,
                            Err(_) => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    Box::<dyn std::error::Error + Send + Sync>::from(
                                        MplCoreError::DeserializationError,
                                    ),
                                ))
                            }
                        }
                }
                // AgentIdentity is a unit variant key (only one per asset).
                ExternalPluginAdapterKey::AgentIdentity => true,
            })
        {
            result = (Some(i), Some(record));
            break;
        }
    }

    Ok(result)
}










#[cfg(test)]
mod parity_tests {
    use super::registry_records_to_plugin_list;
    use borsh::{BorshDeserialize, BorshSerialize};
    use num_traits::FromPrimitive;
    use solana_program::pubkey::Pubkey;

    use crate::*;
    use crate::types::{
        burn_delegate::BurnDelegate,
        permanent_burn_delegate::PermanentBurnDelegate,
        permanent_freeze_delegate::PermanentFreezeDelegate,
        permanent_transfer_delegate::PermanentTransferDelegate,
        transfer_delegate::TransferDelegate,
        Plugin, PluginAuthority, PluginType,
    };

fn registry_records_to_plugin_list_try_fold(
    registry_records: &[RegistryRecordSafe],
    account_data: &[u8],
) -> Result<PluginsList, std::io::Error> {
    let result = registry_records
        .iter()
        .try_fold(PluginsList::default(), |mut acc, record| {
            if PluginType::from_u8(record.plugin_type).is_some() {
                let authority: BaseAuthority = record.authority.clone().into();
                let base = BasePlugin {
                    authority,
                    offset: Some(record.offset),
                };
                let plugin = Plugin::deserialize(&mut &account_data[record.offset as usize..])?;

                match plugin {
                    Plugin::Royalties(royalties) => {
                        acc.royalties = Some(RoyaltiesPlugin { base, royalties });
                    }
                    Plugin::FreezeDelegate(freeze_delegate) => {
                        acc.freeze_delegate = Some(FreezeDelegatePlugin {
                            base,
                            freeze_delegate,
                        });
                    }
                    Plugin::BurnDelegate(burn_delegate) => {
                        acc.burn_delegate = Some(BurnDelegatePlugin {
                            base,
                            burn_delegate,
                        });
                    }
                    Plugin::TransferDelegate(transfer_delegate) => {
                        acc.transfer_delegate = Some(TransferDelegatePlugin {
                            base,
                            transfer_delegate,
                        });
                    }
                    Plugin::UpdateDelegate(update_delegate) => {
                        acc.update_delegate = Some(UpdateDelegatePlugin {
                            base,
                            update_delegate,
                        });
                    }
                    Plugin::PermanentFreezeDelegate(permanent_freeze_delegate) => {
                        acc.permanent_freeze_delegate = Some(PermanentFreezeDelegatePlugin {
                            base,
                            permanent_freeze_delegate,
                        });
                    }
                    Plugin::Attributes(attributes) => {
                        acc.attributes = Some(AttributesPlugin { base, attributes });
                    }
                    Plugin::PermanentTransferDelegate(permanent_transfer_delegate) => {
                        acc.permanent_transfer_delegate = Some(PermanentTransferDelegatePlugin {
                            base,
                            permanent_transfer_delegate,
                        })
                    }
                    Plugin::PermanentBurnDelegate(permanent_burn_delegate) => {
                        acc.permanent_burn_delegate = Some(PermanentBurnDelegatePlugin {
                            base,
                            permanent_burn_delegate,
                        })
                    }
                    Plugin::Edition(edition) => acc.edition = Some(EditionPlugin { base, edition }),
                    Plugin::MasterEdition(master_edition) => {
                        acc.master_edition = Some(MasterEditionPlugin {
                            base,
                            master_edition,
                        })
                    }
                    Plugin::AddBlocker(add_blocker) => {
                        acc.add_blocker = Some(AddBlockerPlugin { base, add_blocker })
                    }
                    Plugin::ImmutableMetadata(immutable_metadata) => {
                        acc.immutable_metadata = Some(ImmutableMetadataPlugin {
                            base,
                            immutable_metadata,
                        })
                    }
                    Plugin::VerifiedCreators(verified_creators) => {
                        acc.verified_creators = Some(VerifiedCreatorsPlugin {
                            base,
                            verified_creators,
                        })
                    }
                    Plugin::Autograph(autograph) => {
                        acc.autograph = Some(AutographPlugin { base, autograph })
                    }
                    Plugin::BubblegumV2(bubblegum_v2) => {
                        acc.bubblegum_v2 = Some(BubblegumV2Plugin { base, bubblegum_v2 })
                    }
                    Plugin::FreezeExecute(freeze_execute) => {
                        acc.freeze_execute = Some(FreezeExecutePlugin {
                            base,
                            freeze_execute,
                        })
                    }
                    Plugin::PermanentFreezeExecute(permanent_freeze_execute) => {
                        acc.permanent_freeze_execute = Some(PermanentFreezeExecutePlugin {
                            base,
                            permanent_freeze_execute,
                        })
                    }
                }
            }
            Ok(acc)
        });

    result
}

// Convert a slice of `AdapterRegistryRecordSafe` into the `ExternalPluginAdaptersList` type, dropping any unknown
// plugins (i.e. `ExternalPluginAdapterType`s that are too new for this client to know about).


    fn assert_same_result(
        orig: &Result<PluginsList, std::io::Error>,
        patched: &Result<PluginsList, std::io::Error>,
    ) {
        assert_eq!(
            orig.is_ok(),
            patched.is_ok(),
            "ok/err classification differed between try_fold and for-loop"
        );
        if let (Ok(o), Ok(p)) = (orig, patched) {
            assert_eq!(
                format!("{:?}", o),
                format!("{:?}", p),
                "PluginsList values differed between try_fold and for-loop"
            );
        }
    }

    fn serialized_plugin(plugin: &Plugin) -> Vec<u8> {
        plugin.try_to_vec().expect("plugin serializes")
    }

    #[test]
    fn empty_registry_returns_default() {
        let records: Vec<RegistryRecordSafe> = vec![];
        let data: Vec<u8> = vec![];
        let orig = registry_records_to_plugin_list_try_fold(&records, &data);
        let patched = registry_records_to_plugin_list(&records, &data);
        assert_same_result(&orig, &patched);
    }

    #[test]
    fn one_known_plugin_populates_correct_field() {
        let plugin = Plugin::PermanentTransferDelegate(PermanentTransferDelegate {});
        let bytes = serialized_plugin(&plugin);
        let mut data = vec![0u8; 16];
        let offset: u64 = 4;
        data[offset as usize..offset as usize + bytes.len()].copy_from_slice(&bytes);

        let records = vec![RegistryRecordSafe {
            plugin_type: PluginType::PermanentTransferDelegate as u8,
            authority: PluginAuthority::Owner,
            offset,
        }];

        let patched = registry_records_to_plugin_list(&records, &data).unwrap();
        let orig = registry_records_to_plugin_list_try_fold(&records, &data).unwrap();

        let ptd = patched.permanent_transfer_delegate.as_ref().unwrap();
        assert_eq!(
            ptd.base,
            BasePlugin {
                authority: BaseAuthority {
                    authority_type: AuthorityType::Owner,
                    address: None,
                },
                offset: Some(offset),
            }
        );
        assert_eq!(ptd.permanent_transfer_delegate, PermanentTransferDelegate {});

        assert_eq!(
            format!("{:?}", patched),
            format!("{:?}", orig),
            "try_fold and for-loop results differed for one known plugin"
        );
    }

    #[test]
    fn multiple_known_plugins_populate_correct_fields() {
        let burn = Plugin::BurnDelegate(BurnDelegate {});
        let transfer = Plugin::TransferDelegate(TransferDelegate {});
        let freeze = Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: false });
        let bytes_burn = serialized_plugin(&burn);
        let bytes_transfer = serialized_plugin(&transfer);
        let bytes_freeze = serialized_plugin(&freeze);

        let mut data = vec![0u8; 32];
        data[0..bytes_burn.len()].copy_from_slice(&bytes_burn);
        data[1..1 + bytes_transfer.len()].copy_from_slice(&bytes_transfer);
        data[2..2 + bytes_freeze.len()].copy_from_slice(&bytes_freeze);

        let records = vec![
            RegistryRecordSafe {
                plugin_type: PluginType::BurnDelegate as u8,
                authority: PluginAuthority::None,
                offset: 0,
            },
            RegistryRecordSafe {
                plugin_type: PluginType::TransferDelegate as u8,
                authority: PluginAuthority::UpdateAuthority,
                offset: 1,
            },
            RegistryRecordSafe {
                plugin_type: PluginType::PermanentFreezeDelegate as u8,
                authority: PluginAuthority::Address { address: Pubkey::default() },
                offset: 2,
            },
        ];

        let patched = registry_records_to_plugin_list(&records, &data).unwrap();
        let orig = registry_records_to_plugin_list_try_fold(&records, &data).unwrap();

        assert_eq!(
            format!("{:?}", patched),
            format!("{:?}", orig),
            "try-fold and for-loop results differed for multiple known plugins"
        );

        assert!(patched.burn_delegate.is_some());
        assert!(patched.transfer_delegate.is_some());
        assert!(patched.permanent_freeze_delegate.is_some());

        assert_eq!(
            patched.burn_delegate.as_ref().unwrap().base.authority,
            BaseAuthority {
                authority_type: AuthorityType::None,
                address: None,
            }
        );
        assert_eq!(
            patched.transfer_delegate.as_ref().unwrap().base.authority,
            BaseAuthority {
                authority_type: AuthorityType::UpdateAuthority,
                address: None,
            }
        );
        assert_eq!(
            patched.permanent_freeze_delegate.as_ref().unwrap().base.authority,
            BaseAuthority {
                authority_type: AuthorityType::Address,
                address: Some(Pubkey::default()),
            }
        );
    }

    #[test]
    fn three_permanent_plugins_are_recognized() {
        let pt = Plugin::PermanentTransferDelegate(PermanentTransferDelegate {});
        let pb = Plugin::PermanentBurnDelegate(PermanentBurnDelegate {});
        let pf = Plugin::PermanentFreezeDelegate(PermanentFreezeDelegate { frozen: false });
        let bytes_pt = serialized_plugin(&pt);
        let bytes_pb = serialized_plugin(&pb);
        let bytes_pf = serialized_plugin(&pf);

        let mut data = vec![0u8; 32];
        data[0..bytes_pt.len()].copy_from_slice(&bytes_pt);
        data[1..1 + bytes_pb.len()].copy_from_slice(&bytes_pb);
        data[2..2 + bytes_pf.len()].copy_from_slice(&bytes_pf);

        let records = vec![
            RegistryRecordSafe {
                plugin_type: PluginType::PermanentTransferDelegate as u8,
                authority: PluginAuthority::Owner,
                offset: 0,
            },
            RegistryRecordSafe {
                plugin_type: PluginType::PermanentBurnDelegate as u8,
                authority: PluginAuthority::Owner,
                offset: 1,
            },
            RegistryRecordSafe {
                plugin_type: PluginType::PermanentFreezeDelegate as u8,
                authority: PluginAuthority::Owner,
                offset: 2,
            },
        ];

        let patched = registry_records_to_plugin_list(&records, &data).unwrap();
        let orig = registry_records_to_plugin_list_try_fold(&records, &data).unwrap();

        assert_eq!(
            format!("{:?}", patched),
            format!("{:?}", orig),
            "try-fold and for-loop results differed for three permanent plugins"
        );

        assert!(patched.permanent_transfer_delegate.is_some());
        assert!(patched.permanent_burn_delegate.is_some());
        assert!(patched.permanent_freeze_delegate.is_some());
    }

    #[test]
    fn unknown_plugin_type_is_skipped() {
        let records = vec![RegistryRecordSafe {
            plugin_type: 99, // not a known PluginType
            authority: PluginAuthority::Owner,
            offset: 0,
        }];
        let data = vec![0u8; 32];

        let orig = registry_records_to_plugin_list_try_fold(&records, &data);
        let patched = registry_records_to_plugin_list(&records, &data);
        assert_same_result(&orig, &patched);

        let patched2 = registry_records_to_plugin_list(&records, &data).unwrap();
        let expected = PluginsList::default();
        assert_eq!(
            format!("{:?}", patched2),
            format!("{:?}", expected),
            "unknown plugin should produce default list"
        );
    }

    #[test]
    fn malformed_plugin_data_returns_error() {
        // PluginType::Royalties (0) with only the discriminant and no inner data.
        let records = vec![RegistryRecordSafe {
            plugin_type: PluginType::Royalties as u8,
            authority: PluginAuthority::Owner,
            offset: 0,
        }];
        let data = vec![0u8; 1];

        let orig = registry_records_to_plugin_list_try_fold(&records, &data);
        let patched = registry_records_to_plugin_list(&records, &data);
        assert_same_result(&orig, &patched);
        // assert_same_result already confirms both are Err.
    }

    #[test]
    fn out_of_range_offset_panics_same_as_upstream() {
        // Upstream 0.11.2 panics when slicing account_data at an offset beyond
        // the slice. This test documents and locks the patched behavior to be
        // identical: both implementations must panic for the same fixture.
        let records = vec![RegistryRecordSafe {
            plugin_type: PluginType::BurnDelegate as u8,
            authority: PluginAuthority::Owner,
            offset: 100,
        }];
        let data = vec![0u8; 4];

        let orig_panics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = registry_records_to_plugin_list_try_fold(&records, &data);
        }))
        .is_err();

        let patched_panics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = registry_records_to_plugin_list(&records, &data);
        }))
        .is_err();

        assert_eq!(
            orig_panics, patched_panics,
            "panic classification differed for out-of-range offset"
        );
        assert!(patched_panics, "patched implementation did not panic as upstream does");
    }
}
