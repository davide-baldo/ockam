use std::time::Duration;

use ockam::identity::models::CredentialAndPurposeKey;
use ockam::identity::TrustEveryonePolicy;
use ockam::identity::Vault;
use ockam::identity::{
    Identifier, Identities, SecureChannelListenerOptions, SecureChannelOptions, SecureChannels,
    TrustMultiIdentifiersPolicy,
};
use ockam::identity::{SecureChannel, SecureChannelListener};
use ockam::{Address, Result, Route};
use ockam_core::api::{Error, Response};
use ockam_core::compat::sync::Arc;
use ockam_core::errcode::{Kind, Origin};
use ockam_core::AsyncTryClone;
use ockam_multiaddr::MultiAddr;
use ockam_node::Context;

use crate::nodes::models::secure_channel::CreateSecureChannelListenerRequest;
use crate::nodes::models::secure_channel::CreateSecureChannelRequest;
use crate::nodes::models::secure_channel::DeleteSecureChannelListenerRequest;
use crate::nodes::models::secure_channel::DeleteSecureChannelRequest;
use crate::nodes::models::secure_channel::ListSecureChannelListenerResponse;
use crate::nodes::models::secure_channel::ShowSecureChannelListenerRequest;
use crate::nodes::models::secure_channel::ShowSecureChannelRequest;
use crate::nodes::models::secure_channel::{
    CreateSecureChannelResponse, DeleteSecureChannelListenerResponse, DeleteSecureChannelResponse,
    ShowSecureChannelListenerResponse, ShowSecureChannelResponse,
};
use crate::nodes::registry::{SecureChannelInfo, SecureChannelListenerInfo};
use crate::nodes::service::default_address::DefaultAddress;
use crate::nodes::{NodeManager, NodeManagerWorker};

/// SECURE CHANNELS
impl NodeManagerWorker {
    pub async fn list_secure_channels(&self) -> Result<Response<Vec<String>>, Response<Error>> {
        Ok(Response::ok().body(self.node_manager.list_secure_channels().await))
    }

    pub(super) async fn create_secure_channel(
        &mut self,
        create_secure_channel: CreateSecureChannelRequest,
        ctx: &Context,
    ) -> Result<Response<CreateSecureChannelResponse>, Response<Error>> {
        let CreateSecureChannelRequest {
            addr,
            authorized_identifiers,
            timeout,
            identity_name: identity,
            credential,
            ..
        } = create_secure_channel;

        let response = self
            .node_manager
            .create_secure_channel(
                ctx,
                addr,
                identity,
                authorized_identifiers,
                credential,
                timeout,
            )
            .await
            .map(|secure_channel| {
                Response::ok().body(CreateSecureChannelResponse::new(secure_channel))
            })?;
        Ok(response)
    }

    pub async fn delete_secure_channel(
        &self,
        delete_secure_channel: DeleteSecureChannelRequest,
        ctx: &Context,
    ) -> Result<Response<DeleteSecureChannelResponse>, Response<Error>> {
        let DeleteSecureChannelRequest {
            channel: address, ..
        } = delete_secure_channel;

        let response = self
            .node_manager
            .delete_secure_channel(ctx, &address)
            .await
            .map(|_| Response::ok().body(DeleteSecureChannelResponse::new(Some(address))))?;
        Ok(response)
    }

    pub async fn show_secure_channel(
        &self,
        show_secure_channel: ShowSecureChannelRequest,
    ) -> Result<Response<ShowSecureChannelResponse>, Response<Error>> {
        let ShowSecureChannelRequest { channel: address } = show_secure_channel;

        let response =
            self.node_manager
                .get_secure_channel(&address)
                .await
                .map(|secure_channel| {
                    Response::ok().body(ShowSecureChannelResponse::new(Some(secure_channel)))
                })?;

        Ok(response)
    }
}

/// SECURE CHANNEL LISTENERS
impl NodeManagerWorker {
    pub async fn create_secure_channel_listener(
        &self,
        create_secure_channel_listener: CreateSecureChannelListenerRequest,
        ctx: &Context,
    ) -> Result<Response<()>, Response<Error>> {
        let CreateSecureChannelListenerRequest {
            addr,
            authorized_identifiers,
            identity_name,
            ..
        } = create_secure_channel_listener;

        let response = self
            .node_manager
            .create_secure_channel_listener(addr, authorized_identifiers, identity_name, ctx)
            .await
            .map(|_| Response::ok())?;
        Ok(response)
    }

    pub async fn delete_secure_channel_listener(
        &self,
        delete_secure_channel_listener: DeleteSecureChannelListenerRequest,
        ctx: &Context,
    ) -> Result<Response<DeleteSecureChannelListenerResponse>, Response<Error>> {
        let DeleteSecureChannelListenerRequest { addr } = delete_secure_channel_listener;

        let response = self
            .node_manager
            .delete_secure_channel_listener(ctx, &addr)
            .await
            .map(|_| Response::ok().body(DeleteSecureChannelListenerResponse::new(addr)))?;
        Ok(response)
    }

    pub async fn show_secure_channel_listener(
        &self,
        show_secure_channel_listener: ShowSecureChannelListenerRequest,
    ) -> Result<Response<ShowSecureChannelListenerResponse>, Response<Error>> {
        let ShowSecureChannelListenerRequest { addr } = show_secure_channel_listener;
        let response = self
            .node_manager
            .get_secure_channel_listener(&addr)
            .await
            .map(|secure_channel_info| {
                Response::ok().body(ShowSecureChannelListenerResponse::new(&secure_channel_info))
            })?;
        Ok(response)
    }

    pub async fn list_secure_channel_listener(
        &self,
    ) -> Result<Response<ListSecureChannelListenerResponse>, Response<Error>> {
        Ok(Response::ok().body(ListSecureChannelListenerResponse::new(
            self.node_manager.list_secure_channel_listeners().await,
        )))
    }
}

/// SECURE CHANNELS
impl NodeManager {
    pub async fn create_secure_channel(
        &self,
        ctx: &Context,
        addr: MultiAddr,
        identity_name: Option<String>,
        authorized_identifiers: Option<Vec<Identifier>>,
        credential: Option<CredentialAndPurposeKey>,
        timeout: Option<Duration>,
    ) -> Result<SecureChannel> {
        let identifier = self.get_identifier_by_name(identity_name.clone()).await?;

        let connection_ctx = Arc::new(ctx.async_try_clone().await?);
        let connection = self
            .make_connection(connection_ctx, &addr, identifier.clone(), None, timeout)
            .await?;
        let sc = self
            .create_secure_channel_internal(
                ctx,
                connection.route()?,
                &identifier,
                authorized_identifiers,
                credential,
                timeout,
            )
            .await?;

        // Return secure channel
        Ok(sc)
    }

    pub(crate) async fn create_secure_channel_internal(
        &self,
        ctx: &Context,
        sc_route: Route,
        identifier: &Identifier,
        authorized_identifiers: Option<Vec<Identifier>>,
        credential: Option<CredentialAndPurposeKey>,
        timeout: Option<Duration>,
    ) -> Result<SecureChannel> {
        debug!(%sc_route, "Creating secure channel");
        let options = SecureChannelOptions::new();

        let options = if let Some(timeout) = timeout {
            options.with_timeout(timeout)
        } else {
            options
        };

        let options = match self.project_authority() {
            Some(project_authority) => options.with_authority(project_authority),
            None => options,
        };

        let options = if let Some(credential) = credential {
            options.with_credential(credential)?
        } else {
            match self.credential_retriever_creators.project_member.as_ref() {
                None => options,
                Some(credential_retriever_creator) => options
                    .with_credential_retriever_creator(credential_retriever_creator.clone())?,
            }
        };

        let options = match authorized_identifiers.clone() {
            Some(ids) => options.with_trust_policy(TrustMultiIdentifiersPolicy::new(ids)),
            None => options.with_trust_policy(TrustEveryonePolicy),
        };

        let sc = self
            .secure_channels
            .create_secure_channel(ctx, identifier, sc_route.clone(), options)
            .await?;

        debug!(%sc_route, %sc, "Created secure channel");

        self.registry
            .secure_channels
            .insert(sc_route, sc.clone(), authorized_identifiers)
            .await;

        Ok(sc)
    }

    pub async fn delete_secure_channel(&self, ctx: &Context, addr: &Address) -> Result<()> {
        debug!(%addr, "deleting secure channel");
        if (self.registry.secure_channels.get_by_addr(addr).await).is_none() {
            return Err(ockam_core::Error::new(
                Origin::Api,
                Kind::NotFound,
                format!("Secure channel with address, {}, not found", addr),
            ));
        }
        self.secure_channels.stop_secure_channel(ctx, addr).await?;
        self.registry.secure_channels.remove_by_addr(addr).await;
        Ok(())
    }

    pub async fn get_secure_channel(&self, addr: &Address) -> Result<SecureChannelInfo> {
        debug!(%addr, "On show secure channel");
        self.registry
            .secure_channels
            .get_by_addr(addr)
            .await
            .ok_or(ockam_core::Error::new(
                Origin::Api,
                Kind::NotFound,
                format!("Secure channel with address, {}, not found", addr),
            ))
    }

    pub async fn list_secure_channels(&self) -> Vec<String> {
        let registry = &self.registry.secure_channels;
        let secure_channel_list = registry.list().await;
        secure_channel_list
            .into_iter()
            .map(|secure_channel| secure_channel.sc().encryptor_address().to_string())
            .collect()
    }
}

/// SECURE CHANNEL LISTENERS
impl NodeManager {
    pub async fn create_secure_channel_listener(
        &self,
        address: Address,
        authorized_identifiers: Option<Vec<Identifier>>,
        identity_name: Option<String>,
        ctx: &Context,
    ) -> Result<SecureChannelListener> {
        debug!(
            "Handling request to create a new secure channel listener: {}",
            address
        );

        let named_identity = match identity_name {
            Some(identity_name) => self.cli_state.get_named_identity(&identity_name).await?,
            None => {
                self.cli_state
                    .get_named_identity_by_identifier(&self.identifier())
                    .await?
            }
        };
        let identifier = named_identity.identifier();
        let vault = self
            .cli_state
            .get_named_vault(&named_identity.vault_name())
            .await?
            .vault()
            .await?;
        let secure_channels = self.build_secure_channels(vault).await?;

        let options =
            SecureChannelListenerOptions::new().as_consumer(&self.api_transport_flow_control_id);

        let options = match authorized_identifiers {
            Some(ids) => options.with_trust_policy(TrustMultiIdentifiersPolicy::new(ids)),
            None => options.with_trust_policy(TrustEveryonePolicy),
        };

        let options = match self.project_authority() {
            Some(project_authority) => options.with_authority(project_authority),
            None => options,
        };

        let options = match self.credential_retriever_creators.project_member.as_ref() {
            None => options,
            Some(credential_retriever_creator) => {
                options.with_credential_retriever_creator(credential_retriever_creator.clone())?
            }
        };

        let listener = secure_channels
            .create_secure_channel_listener(ctx, &identifier, address.clone(), options)
            .await?;

        self.registry
            .secure_channel_listeners
            .insert(
                address.clone(),
                SecureChannelListenerInfo::new(listener.clone()),
            )
            .await;

        // TODO: Clean
        // Add Echoer, Uppercase and Cred Exch as a consumer by default
        ctx.flow_controls()
            .add_consumer(DefaultAddress::ECHO_SERVICE, listener.flow_control_id());

        ctx.flow_controls().add_consumer(
            DefaultAddress::UPPERCASE_SERVICE,
            listener.flow_control_id(),
        );

        Ok(listener)
    }

    pub async fn delete_secure_channel_listener(
        &self,
        ctx: &Context,
        addr: &Address,
    ) -> Result<SecureChannelListenerInfo> {
        debug!("deleting secure channel listener: {addr}");
        ctx.stop_worker(addr.clone()).await?;
        self.registry
            .secure_channel_listeners
            .remove(addr)
            .await
            .ok_or(ockam_core::Error::new(
                Origin::Api,
                Kind::Internal,
                format!("Error while deleting secure channel with addrress {}", addr,),
            ))
    }

    pub async fn get_secure_channel_listener(
        &self,
        addr: &Address,
    ) -> Result<SecureChannelListenerInfo> {
        debug!(%addr, "On show secure channel listener");
        self.registry
            .secure_channel_listeners
            .get(addr)
            .await
            .ok_or(ockam_core::Error::new(
                Origin::Api,
                Kind::NotFound,
                format!("Secure channel with address, {}, not found", addr),
            ))
    }

    pub async fn list_secure_channel_listeners(&self) -> Vec<SecureChannelListenerInfo> {
        let registry = &self.registry.secure_channel_listeners;
        registry.values().await
    }
}

impl NodeManager {
    /// Build a SecureChannels struct for a specific vault
    pub(crate) async fn build_secure_channels(&self, vault: Vault) -> Result<Arc<SecureChannels>> {
        let identities = Identities::create_with_node(self.cli_state.database(), &self.node_name)
            .with_vault(vault)
            .build();
        Ok(Arc::new(SecureChannels::new(
            identities,
            self.secure_channels.secure_channel_registry(),
        )))
    }
}
