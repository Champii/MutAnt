use mutant_protocol::{
    GetCallback as ProtocolGetCallback, GetEvent as ProtocolGetEvent,
    HealthCheckCallback as ProtocolHealthCheckCallback,
    HealthCheckEvent as ProtocolHealthCheckEvent, PurgeCallback as ProtocolPurgeCallback,
    PurgeEvent as ProtocolPurgeEvent, PutCallback as ProtocolPutCallback,
    PutEvent as ProtocolPutEvent, SyncCallback as ProtocolSyncCallback,
    SyncEvent as ProtocolSyncEvent,
};

use crate::error::Error;

pub async fn invoke_put_callback(
    callback: &Option<ProtocolPutCallback>,
    event: ProtocolPutEvent,
) -> Result<bool, Error> {
    if let Some(cb) = callback {
        match cb(event).await {
            Ok(continue_op) => Ok(continue_op),
            Err(e) => Err(Error::CallbackError(e.to_string())),
        }
    } else {
        Ok(true)
    }
}

pub(crate) async fn invoke_get_callback(
    callback: &Option<ProtocolGetCallback>,
    event: ProtocolGetEvent,
) -> Result<bool, Error> {
    if let Some(cb) = callback {
        match cb(event).await {
            Ok(continue_op) => Ok(continue_op),
            Err(e) => Err(Error::CallbackError(e.to_string())),
        }
    } else {
        Ok(true)
    }
}

pub(crate) async fn invoke_purge_callback(
    callback: &Option<ProtocolPurgeCallback>,
    event: ProtocolPurgeEvent,
) -> Result<bool, Error> {
    if let Some(cb) = callback {
        match cb(event).await {
            Ok(continue_op) => Ok(continue_op),
            Err(e) => Err(Error::CallbackError(e.to_string())),
        }
    } else {
        Ok(true)
    }
}

pub(crate) async fn invoke_sync_callback(
    callback: &Option<ProtocolSyncCallback>,
    event: ProtocolSyncEvent,
) -> Result<bool, Error> {
    if let Some(cb) = callback {
        match cb(event).await {
            Ok(continue_op) => Ok(continue_op),
            Err(e) => Err(Error::CallbackError(e.to_string())),
        }
    } else {
        Ok(true)
    }
}

pub(crate) async fn invoke_health_check_callback(
    callback: &Option<ProtocolHealthCheckCallback>,
    event: ProtocolHealthCheckEvent,
) -> Result<bool, Error> {
    if let Some(cb) = callback {
        match cb(event).await {
            Ok(continue_op) => Ok(continue_op),
            Err(e) => Err(Error::CallbackError(e.to_string())),
        }
    } else {
        Ok(true)
    }
}
