//! The Fat Contract registry

pub use self::pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use codec::Encode;
	use frame_support::{dispatch::DispatchResult, pallet_prelude::*, traits::StorageVersion};
	use frame_system::pallet_prelude::*;
	use sp_core::H256;
	use sp_runtime::AccountId32;
	use sp_std::prelude::*;

	use crate::{mq::MessageOriginInfo, registry};
	// Re-export
	pub use crate::attestation::{Attestation, IasValidator};
	use phala_types::{
		contract::{
			ClusterInfo, ClusterPermission, CodeIndex, ContractClusterId, ContractId, ContractInfo, 
messaging::{ClusterEvent, ContractOperation, ClusterOperation, WorkerClusterReport, WorkerContractReport, ResourceType},
		},
		messaging::{
			bind_topic, DecodedMessage, MessageOrigin,
		},
		ClusterPublicKey, ContractPublicKey, WorkerIdentity, WorkerPublicKey,
	};

	bind_topic!(ClusterRegistryEvent, b"^phala/registry/cluster");
	#[derive(Encode, Decode, Clone, Debug)]
	pub enum ClusterRegistryEvent {
		PubkeyAvailable {
			cluster: ContractClusterId,
			pubkey: ClusterPublicKey,
		},
	}

	bind_topic!(ContractRegistryEvent, b"^phala/registry/contract");
	#[derive(Encode, Decode, Clone, Debug)]
	pub enum ContractRegistryEvent {
		PubkeyAvailable {
			contract: ContractId,
			pubkey: ContractPublicKey,
		},
	}

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
	}

	const STORAGE_VERSION: StorageVersion = StorageVersion::new(5);

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::storage_version(STORAGE_VERSION)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::storage]
	pub type Contracts<T: Config> =
		StorageMap<_, Twox64Concat, ContractId, ContractInfo<CodeHash<T>, T::AccountId>>;

	/// The contract cluster counter, it always equals to the latest cluster id.
	#[pallet::storage]
	pub type ClusterCounter<T> = StorageValue<_, u64, ValueQuery>;

	#[pallet::storage]
	pub type Clusters<T: Config> =
		StorageMap<_, Twox64Concat, ContractClusterId, ClusterInfo<T::AccountId>>;

	#[pallet::storage]
	pub type ClusterContracts<T: Config> =
		StorageMap<_, Twox64Concat, ContractClusterId, Vec<ContractId>, ValueQuery>;

	#[pallet::storage]
	pub type ClusterWorkers<T> =
		StorageMap<_, Twox64Concat, ContractClusterId, Vec<WorkerPublicKey>, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		ClusterCreated {
			cluster: ContractClusterId,
		},
		ClusterPubkeyAvailable {
			cluster: ContractClusterId,
			pubkey: ClusterPublicKey,
		},
		ClusterDeployed {
			cluster: ContractClusterId,
			pubkey: ClusterPublicKey,
			worker: WorkerPublicKey,
		},
		ClusterDeploymentFailed {
			cluster: ContractClusterId,
			worker: WorkerPublicKey,
		},
		Instantiating {
			contract: ContractId,
			cluster: ContractClusterId,
			deployer: T::AccountId,
		},
		ContractPubkeyAvailable {
			contract: ContractId,
			cluster: ContractClusterId,
			pubkey: ContractPublicKey,
		},
		Instantiated {
			contract: ContractId,
			cluster: ContractClusterId,
			deployer: H256,
		},
		InstantiationFailed {
			contract: ContractId,
			cluster: ContractClusterId,
			deployer: H256,
		},
		ClusterSetLogReceiver {
			cluster: ContractClusterId,
			log_handler: ContractId,
		},
		ClusterDestroyed {
			cluster: ContractClusterId,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		CodeNotFound,
		ClusterNotFound,
		ClusterNotDeployed,
		ClusterPermissionDenied,
		DuplicatedContract,
		DuplicatedDeployment,
		NoWorkerSpecified,
		InvalidSender,
		WorkerNotFound,
	}

	type CodeHash<T> = <T as frame_system::Config>::Hash;

	fn check_cluster_permission<T: Config>(
		deployer: &T::AccountId,
		cluster: &ClusterInfo<T::AccountId>,
	) -> bool {
		match &cluster.permission {
			ClusterPermission::Public => true,
			ClusterPermission::OnlyOwner(owner) => deployer == owner,
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T>
	where
		T: crate::mq::Config + crate::registry::Config,
		T: frame_system::Config<AccountId = AccountId32>,
	{
		#[pallet::weight(0)]
		pub fn add_cluster(
			origin: OriginFor<T>,
			permission: ClusterPermission<T::AccountId>,
			deploy_workers: Vec<WorkerPublicKey>,
		) -> DispatchResult {
			// TODO.shelven: permission check?
			let origin: T::AccountId = ensure_signed(origin)?;

			ensure!(deploy_workers.len() > 0, Error::<T>::NoWorkerSpecified);
			let workers = deploy_workers
				.iter()
				.map(|worker| {
					let worker_info =
						registry::Workers::<T>::get(worker).ok_or(Error::<T>::WorkerNotFound)?;
					Ok(WorkerIdentity {
						pubkey: worker_info.pubkey,
						ecdh_pubkey: worker_info.ecdh_pubkey,
					})
				})
				.collect::<Result<Vec<WorkerIdentity>, Error<T>>>()?;

			let cluster_info = ClusterInfo {
				owner: origin,
				permission,
				workers: deploy_workers,
			};

			let cluster_id = ClusterCounter::<T>::mutate(|counter| {
				let cluster_id = *counter;
				*counter += 1;
				cluster_id
			});
			let cluster = ContractClusterId::from_low_u64_be(cluster_id);

			Clusters::<T>::insert(&cluster, &cluster_info);
			Self::deposit_event(Event::ClusterCreated { cluster });
			Self::push_message(ClusterEvent::DeployCluster { cluster, workers });
			Ok(())
		}

		#[pallet::weight(0)]
		pub fn cluster_upload_resource(
			origin: OriginFor<T>,
			cluster_id: ContractClusterId,
			resource_type: ResourceType,
			resource_data: Vec<u8>,
		) -> DispatchResult {
			let origin: T::AccountId = ensure_signed(origin)?;
			let cluster_info = Clusters::<T>::get(cluster_id).ok_or(Error::<T>::ClusterNotFound)?;
			ensure!(
				check_cluster_permission::<T>(&origin, &cluster_info),
				Error::<T>::ClusterPermissionDenied
			);

			Self::push_message(ClusterOperation::<_, T::BlockNumber>::UploadResource {
				origin,
				cluster_id,
				resource_type,
				resource_data,
			});
			Ok(())
		}

		#[pallet::weight(0)]
		pub fn instantiate_contract(
			origin: OriginFor<T>,
			code_index: CodeIndex<CodeHash<T>>,
			data: Vec<u8>,
			salt: Vec<u8>,
			cluster_id: ContractClusterId,
		) -> DispatchResult {
			let deployer = ensure_signed(origin)?;
			let cluster_info = Clusters::<T>::get(cluster_id).ok_or(Error::<T>::ClusterNotFound)?;
			ensure!(
				check_cluster_permission::<T>(&deployer, &cluster_info),
				Error::<T>::ClusterPermissionDenied
			);

			let contract_info = ContractInfo {
				deployer,
				code_index,
				salt,
				cluster_id,
				instantiate_data: data,
			};
			let contract_id = contract_info.contract_id(crate::hashing::blake2_256);
			ensure!(
				!Contracts::<T>::contains_key(contract_id),
				Error::<T>::DuplicatedContract
			);
			Contracts::<T>::insert(&contract_id, &contract_info);

			Self::push_message(ContractOperation::instantiate_code(contract_info.clone()));
			Self::deposit_event(Event::Instantiating {
				contract: contract_id,
				cluster: contract_info.cluster_id,
				deployer: contract_info.deployer,
			});

			Ok(())
		}

		#[pallet::weight(0)]
		pub fn cluster_set_log_handler(
			origin: OriginFor<T>,
			cluster: ContractClusterId,
			log_handler: ContractId,
		) -> DispatchResult {
			let origin = ensure_signed(origin)?;
			let cluster_info = Clusters::<T>::get(&cluster).ok_or(Error::<T>::ClusterNotFound)?;
			ensure!(
				origin == cluster_info.owner,
				Error::<T>::ClusterPermissionDenied
			);

			Self::push_message(
				ClusterOperation::<T::AccountId, T::BlockNumber>::SetLogReceiver {
					cluster,
					log_handler,
				},
			);
			Self::deposit_event(Event::ClusterSetLogReceiver {
				cluster,
				log_handler,
			});
			Ok(())
		}

		#[pallet::weight(0)]
		pub fn cluster_destroy(origin: OriginFor<T>, cluster: ContractClusterId) -> DispatchResult {
			ensure_root(origin)?;

			Clusters::<T>::take(&cluster).ok_or(Error::<T>::ClusterNotFound)?;
			Self::push_message(ClusterOperation::<T::AccountId, T::BlockNumber>::DestroyCluster(cluster));
			Self::deposit_event(Event::ClusterDestroyed { cluster });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T>
	where
		T: crate::mq::Config + crate::registry::Config,
	{
		pub fn on_cluster_message_received(
			message: DecodedMessage<ClusterRegistryEvent>,
		) -> DispatchResult {
			ensure!(
				message.sender == MessageOrigin::Gatekeeper,
				Error::<T>::InvalidSender
			);
			match message.payload {
				ClusterRegistryEvent::PubkeyAvailable { cluster, pubkey } => {
					// The cluster key can be over-written with the latest value by Gatekeeper
					registry::ClusterKeys::<T>::insert(&cluster, &pubkey);
					Self::deposit_event(Event::ClusterPubkeyAvailable { cluster, pubkey });
				}
			}
			Ok(())
		}

		pub fn on_contract_message_received(
			message: DecodedMessage<ContractRegistryEvent>,
		) -> DispatchResult {
			let cluster = match message.sender {
				MessageOrigin::Cluster(cluster) => cluster,
				_ => return Err(Error::<T>::InvalidSender.into()),
			};
			match message.payload {
				ContractRegistryEvent::PubkeyAvailable { contract, pubkey } => {
					registry::ContractKeys::<T>::insert(&contract, &pubkey);
					Self::deposit_event(Event::ContractPubkeyAvailable {
						contract,
						cluster,
						pubkey,
					});
				}
			}
			Ok(())
		}

		pub fn on_worker_cluster_message_received(
			message: DecodedMessage<WorkerClusterReport>,
		) -> DispatchResult {
			let worker_pubkey = match message.sender {
				MessageOrigin::Worker(worker_pubkey) => worker_pubkey,
				_ => return Err(Error::<T>::InvalidSender.into()),
			};
			match message.payload {
				WorkerClusterReport::ClusterDeployed { id, pubkey } => {
					// TODO.shelven: scalability concern for large number of workers
					ClusterWorkers::<T>::append(&id, &worker_pubkey);
					Self::deposit_event(Event::ClusterDeployed {
						cluster: id,
						pubkey,
						worker: worker_pubkey,
					});
				}
				WorkerClusterReport::ClusterDeploymentFailed { id } => {
					Self::deposit_event(Event::ClusterDeploymentFailed {
						cluster: id,
						worker: worker_pubkey,
					});
				}
			}
			Ok(())
		}

		pub fn on_worker_contract_message_received(
			message: DecodedMessage<WorkerContractReport>,
		) -> DispatchResult {
			let _worker_pubkey = match &message.sender {
				MessageOrigin::Worker(worker_pubkey) => worker_pubkey,
				_ => return Err(Error::<T>::InvalidSender.into()),
			};
			match message.payload {
				WorkerContractReport::ContractInstantiated {
					id,
					cluster_id,
					deployer,
					pubkey: _,
				} => {
					let contracts = ClusterContracts::<T>::get(&cluster_id);
					if !contracts.contains(&id) {
						ClusterContracts::<T>::append(&cluster_id, &id);
					}
					Self::deposit_event(Event::Instantiated {
						contract: id,
						cluster: cluster_id,
						deployer,
					});
				}
				WorkerContractReport::ContractInstantiationFailed {
					id,
					cluster_id,
					deployer,
				} => {
					Self::deposit_event(Event::InstantiationFailed {
						contract: id,
						cluster: cluster_id,
						deployer,
					});
					// TODO.shelven: some cleanup?
				}
			}
			Ok(())
		}
	}

	impl<T: Config + crate::mq::Config> MessageOriginInfo for Pallet<T> {
		type Config = T;
	}
}
