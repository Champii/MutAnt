#[macro_export]
macro_rules! direct_request {
    ($self:expr, $key:ident, $($req:tt)*) => {{
        let key = PendingRequestKey::$key;
        let req = Request::$key(mutant_protocol::$($req)*);

        if $self.pending_requests.lock().unwrap().contains_key(&key) {
            return Err(ClientError::InternalError(
                "Another list_keys request is already pending".to_string(),
            ));
        } else {
            let (sender, receiver) = oneshot::channel();
            let pending_sender = PendingSender::$key(sender);

            $self
                .pending_requests
                .lock()
                .unwrap()
                .insert(key.clone(), pending_sender);

            match $self.send_request(req).await {
                Ok(_) => {
                    debug!("{} request sent, waiting for response...", stringify!($key));
                    match receiver.await {
                        Ok(result) => result,
                        Err(_) => {
                            $self.pending_requests.lock().unwrap().remove(&key);
                            error!("{} response channel canceled", stringify!($key));
                            Err(ClientError::InternalError(
                                "{} response channel canceled".to_string(),
                            ))
                        }
                    }
                }
                Err(e) => {
                    $self.pending_requests.lock().unwrap().remove(&key);
                    error!("Failed to send {} request: {:?}", stringify!($key), e);
                    Err(e)
                }
            }
        }
    }};
}

#[macro_export]
macro_rules! long_request {
    ($self:expr, $key:ident, $($req:tt)*) => {{
        let key = PendingRequestKey::TaskCreation;
        let req = Request::$key(mutant_protocol::$($req)*);
        if $self.pending_requests.lock().unwrap().contains_key(&key) {
            return Err(ClientError::InternalError(
                "Another put/get request is already pending".to_string(),
            ));
        }

        let (completion_tx, completion_rx) = oneshot::channel();
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();

        let (task_creation_tx, task_creation_rx) = oneshot::channel();
        $self.pending_requests.lock().unwrap().insert(
            key.clone(),
            PendingSender::TaskCreation(
                task_creation_tx,
                (completion_tx, progress_tx),
                TaskType::$key,
            ),
        );

        let start_task = async move {
            match $self.send_request(req).await {
                Ok(_) => {
                    debug!("{} request sent, waiting for TaskCreated response...", stringify!($key));
                    let task_id = task_creation_rx.await.map_err(|_| {
                        ClientError::InternalError("TaskCreated channel canceled".to_string())
                    })??;

                    info!("Task created with ID: {}", task_id);

                    completion_rx.await.map_err(|_| {
                        error!("Completion channel canceled");
                        ClientError::InternalError("Completion channel canceled".to_string())
                    })?
                }
                Err(e) => {
                    error!("Failed to send {} request: {:?}", stringify!($key), e);
                    $self.pending_requests.lock().unwrap().remove(&key);
                    Err(e)
                }
            }
        };

        Ok((start_task, progress_rx))

    }}
}
