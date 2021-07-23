pub use self::pallet::*;

#[allow(unused_variables)]
#[frame_support::pallet]
pub mod pallet {
	use crate::mining;
	use crate::registry;
	use frame_support::{
		dispatch::DispatchResult,
		pallet_prelude::*,
		traits::{
			Currency, EnsureOrigin, LockIdentifier, LockableCurrency, UnixTime, WithdrawReasons,
		},
		PalletId,
	};
	use frame_system::pallet_prelude::*;

	use phala_types::{messaging::SettleInfo, WorkerPublicKey};
	use sp_runtime::{
		traits::{AccountIdConversion, Saturating, TrailingZeroInput, Zero},
		Permill, SaturatedConversion,
	};
	use sp_std::collections::vec_deque::VecDeque;
	use sp_std::vec;
	use sp_std::vec::Vec;

	const STAKEPOOL_PALLETID: PalletId = PalletId(*b"phala/sp");
	const STAKING_ID: LockIdentifier = *b"phala/sp";

	pub trait Ledger<AccountId, Balance> {
		/// Increases the locked amount for a user
		///
		/// Unsafe: it assumes there's enough free `amount`
		fn ledger_accrue(who: &AccountId, amount: Balance);
		/// Decreases the locked amount for a user
		///
		/// Unsafe: it assumes there's enough locked `amount`
		fn ledger_reduce(who: &AccountId, amount: Balance);
		/// Gets the locked amount of `who`
		fn ledger_query(who: &AccountId) -> Balance;
	}

	#[pallet::config]
	pub trait Config: frame_system::Config + registry::Config + mining::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		type Currency: LockableCurrency<Self::AccountId, Moment = Self::BlockNumber>;
		type MinDeposit: Get<BalanceOf<Self>>;
		type InsurancePeriod: Get<Self::BlockNumber>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	/// Mapping from pool id to PoolInfo
	#[pallet::storage]
	#[pallet::getter(fn mining_pools)]
	pub(super) type MiningPools<T: Config> =
		StorageMap<_, Twox64Concat, u64, PoolInfo<T::AccountId, BalanceOf<T>>>;

	/// Mapping pool to it's UserStakeInfo
	#[pallet::storage]
	#[pallet::getter(fn staking_info)]
	pub(super) type StakingInfo<T: Config> =
		StorageMap<_, Twox64Concat, (u64, T::AccountId), UserStakeInfo<T::AccountId, BalanceOf<T>>>;

	#[pallet::storage]
	#[pallet::getter(fn pool_count)]
	pub(super) type PoolCount<T> = StorageValue<_, u64, ValueQuery>;

	/// Mapping worker to the pool it belongs to
	#[pallet::storage]
	pub(super) type WorkerInPool<T: Config> = StorageMap<_, Twox64Concat, WorkerPublicKey, u64>;

	/// Mapping staker to it's the balance locked in all pools
	#[pallet::storage]
	#[pallet::getter(fn stake_ledger)]
	pub(super) type StakeLedger<T: Config> =
		StorageMap<_, Twox64Concat, T::AccountId, BalanceOf<T>>;

	/// Mapping current block timestamp to pools that contains waiting withdraw request in that block
	#[pallet::storage]
	#[pallet::getter(fn withdraw_pools)]
	pub(super) type WithdrawPools<T: Config> = StorageMap<_, Twox64Concat, u64, Vec<u64>>;

	/// Queue that contains all block's timestamp, in that block contains the waiting withdraw reqeust.
	/// This queue has a max size of (T::InsurancePeriod * 8) bytes
	#[pallet::storage]
	#[pallet::getter(fn withdraw_timestamps)]
	pub(super) type WithdrawTimestamps<T> = StorageValue<_, VecDeque<u64>, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Meh. [n]
		Meh(u32),
		/// [owner, pid]
		PoolCreated(T::AccountId, u64),
		/// [pid, commission]
		PoolCommissionSet(u64, Permill),
		/// [pid, cap]
		PoolCapacitySet(u64, BalanceOf<T>),
		/// [pid, worker]
		PoolWorkerAdded(u64, WorkerPublicKey),
		/// [pid, user, amount]
		Deposit(u64, T::AccountId, BalanceOf<T>),
		/// [pid, user, amount]
		Withdraw(u64, T::AccountId, BalanceOf<T>),
		/// [pid, user, amount]
		WithdrawRewards(u64, T::AccountId, BalanceOf<T>),
	}

	#[pallet::error]
	pub enum Error<T> {
		Meh,
		WorkerNotRegistered,
		BenchmarkMissing,
		WorkerHasAdded,
		WorkerHasNotAdded,
		UnauthorizedOperator,
		UnauthorizedPoolOwner,
		InvalidCapacity,
		StakeExceedCapacity,
		PoolNotExist,
		PoolIsBusy,
		LessThanMinDeposit,
		InsufficientBalance,
		StakeInfoNotFound,
		InsufficientStake,
		InvalidWithdrawAmount,
		StartMiningCallFailed,
		MinerBindingCallFailed,
	}

	type BalanceOf<T> =
		<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	#[pallet::hooks]
	impl<T: Config> Hooks<T::BlockNumber> for Pallet<T> {
		fn on_finalize(_n: T::BlockNumber) {
			let now = <T as registry::Config>::UnixTime::now()
				.as_secs()
				.saturated_into::<u64>();

			let mut t = WithdrawTimestamps::<T>::get();
			// has waiting withdraw request
			if !t.is_empty() {
				// we just handle timeout request every block
				while let Some(start_time) = t.front().cloned() {
					if now - start_time <= T::InsurancePeriod::get().saturated_into::<u64>() {
						break;
					}
					let pools =
						WithdrawPools::<T>::take(start_time).expect("Pool list must exist; qed.");
					for &pid in pools.iter() {
						let pool_info =
							Self::ensure_pool(pid).expect("Stake pool must exist; qed.");
						// if we check the pool withdraw_queue here, we don't have to remove a pool from WithdrawPools when
						// a pool has handled their waiting withdraw request before timeout. Compare the IO performance we
						// think remove pool from WithdrawPools would have more resource cost.
						if !pool_info.withdraw_queue.is_empty() {
							// the front withdraw always the oldest one
							if let Some(info) = pool_info.withdraw_queue.front() {
								if (now - info.start_time)
									> T::InsurancePeriod::get().saturated_into::<u64>()
								{
									// stop all worker in this pool
									// TODO: only stop running workers?
									for worker in pool_info.workers {
										let miner: T::AccountId = pool_sub_account(pid, &worker);
										// TODO: avoid stop mining multiple times
										let _ = <mining::pallet::Pallet<T>>::stop_mining(miner);
									}
								}
							}
						}
					}
					// pop front timestamp
					t.pop_front();
				}
				WithdrawTimestamps::<T>::put(&t);
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T>
	where
		T: mining::Config<Currency = <T as Config>::Currency>,
	{
		/// Creates a new stake pool
		#[pallet::weight(0)]
		pub fn create(origin: OriginFor<T>) -> DispatchResult {
			let owner = ensure_signed(origin)?;

			let pid = PoolCount::<T>::get();
			MiningPools::<T>::insert(
				pid,
				PoolInfo {
					pid: pid,
					owner: owner.clone(),
					payout_commission: None,
					owner_reward: Zero::zero(),
					cap: None,
					pool_acc: Zero::zero(),
					total_stake: Zero::zero(),
					free_stake: Zero::zero(),
					workers: vec![],
					withdraw_queue: VecDeque::new(),
				},
			);
			PoolCount::<T>::put(pid + 1);
			Self::deposit_event(Event::<T>::PoolCreated(owner, pid));

			Ok(())
		}

		#[pallet::weight(0)]
		pub fn add_worker(
			origin: OriginFor<T>,
			pid: u64,
			pubkey: WorkerPublicKey,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let worker_info =
				registry::Worker::<T>::get(&pubkey).ok_or(Error::<T>::WorkerNotRegistered)?;

			// check wheather the owner was bounded as operator
			ensure!(
				worker_info.operator == Some(owner.clone()),
				Error::<T>::UnauthorizedOperator
			);
			// check the worker has finished the benchmark
			ensure!(
				worker_info.intial_score != None,
				Error::<T>::BenchmarkMissing
			);

			// origin must be owner of pool
			let mut pool_info = Self::ensure_pool(pid)?;
			ensure!(pool_info.owner == owner, Error::<T>::UnauthorizedPoolOwner);
			// make sure worker has not been not added
			// TODO: should we set a cap to avoid performance problem
			let workers = &mut pool_info.workers;
			// TODO: limit the number of workers to avoid performance issue.
			ensure!(!workers.contains(&pubkey), Error::<T>::WorkerHasAdded);

			// generate miner account
			let miner: T::AccountId = pool_sub_account(pid, &pubkey);

			// bind worker with minner
			mining::pallet::Pallet::<T>::bind(miner.clone(), pubkey.clone())
				.or(Err(Error::<T>::MinerBindingCallFailed))?;

			// update worker vector
			workers.push(pubkey.clone());
			MiningPools::<T>::insert(&pid, &pool_info);
			WorkerInPool::<T>::insert(&pubkey, pid);
			Self::deposit_event(Event::<T>::PoolWorkerAdded(pid, pubkey));

			Ok(())
		}

		/// Destroies a stake pool
		///
		/// Requires:
		/// 1. The sender is the owner
		/// 2. All the miners are stopped
		#[pallet::weight(0)]
		pub fn destroy(origin: OriginFor<T>, id: u64) -> DispatchResult {
			panic!("unimplemented")
		}

		/// Sets the hard cap of the pool
		/// Note: a smaller cap than current total_stake if not allowed.
		/// Requires:
		/// 1. The sender is the owner
		#[pallet::weight(0)]
		pub fn set_cap(origin: OriginFor<T>, pid: u64, cap: BalanceOf<T>) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let mut pool_info = Self::ensure_pool(pid)?;

			// origin must be owner of pool
			ensure!(pool_info.owner == owner, Error::<T>::UnauthorizedPoolOwner);
			// check cap
			ensure!(pool_info.total_stake <= cap, Error::<T>::InvalidCapacity);

			pool_info.cap = Some(cap);
			MiningPools::<T>::insert(&pid, &pool_info);

			Self::deposit_event(Event::<T>::PoolCapacitySet(pid, cap));
			Ok(())
		}

		/// Change the pool commission rate
		///
		/// Requires:
		/// 1. The sender is the owner
		#[pallet::weight(0)]
		pub fn set_payout_pref(
			origin: OriginFor<T>,
			pid: u64,
			payout_commission: Permill,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let mut pool_info = Self::ensure_pool(pid)?;
			// origin must be owner of pool
			ensure!(pool_info.owner == owner, Error::<T>::UnauthorizedPoolOwner);

			pool_info.payout_commission = Some(payout_commission);
			MiningPools::<T>::insert(&pid, &pool_info);

			Self::deposit_event(Event::<T>::PoolCommissionSet(pid, payout_commission));

			Ok(())
		}

		/// Change the payout perference of a stake pool
		///
		/// Requires:
		/// 1. The sender is the owner
		#[pallet::weight(0)]
		pub fn claim_reward(
			origin: OriginFor<T>,
			pid: u64,
			target: T::AccountId,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let info_key = (pid.clone(), who.clone());
			let mut user_info =
				Self::staking_info(&info_key).ok_or(Error::<T>::StakeInfoNotFound)?;
			let pool_info = Self::ensure_pool(pid)?;

			// Clear the pending reward, and calculate the rewards belong to user
			pool_info.clear_user_pending_reward(&mut user_info);
			let rewards = user_info.available_rewards;
			// TODO: transfer token from the pallet to the user, instead of creating imbalance.
			<T as Config>::Currency::deposit_into_existing(&target, rewards.clone())?;
			user_info.available_rewards = Zero::zero();

			StakingInfo::<T>::insert(&info_key, &user_info);
			Self::deposit_event(Event::<T>::WithdrawRewards(pid, who, rewards));

			Ok(())
		}

		/// Deposits some funds to a pool
		///
		/// Requires:
		/// 1. The pool exists
		/// 2. After the desposit, the pool doesn't reach the cap
		#[pallet::weight(0)]
		pub fn deposit(origin: OriginFor<T>, pid: u64, amount: BalanceOf<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let a = amount; // Alias to reduce confusion in the code below

			ensure!(a >= T::MinDeposit::get(), Error::<T>::LessThanMinDeposit);
			ensure!(
				<T as Config>::Currency::free_balance(&who) >= a,
				Error::<T>::InsufficientBalance
			);

			let mut pool_info = Self::ensure_pool(pid)?;
			if let Some(cap) = pool_info.cap {
				ensure!(
					cap.saturating_sub(pool_info.total_stake) >= a,
					Error::<T>::StakeExceedCapacity
				);
			}

			let info_key = (pid.clone(), who.clone());
			// Clear the pending reward before adding stake, if applies
			let mut user_info = match Self::staking_info(&info_key) {
				Some(mut user_info) => {
					pool_info.clear_user_pending_reward(&mut user_info);
					user_info
				}
				None => UserStakeInfo {
					user: who.clone(),
					amount: Zero::zero(),
					available_rewards: Zero::zero(),
					user_debt: Zero::zero(),
				},
			};
			// Add the stake
			user_info.amount.saturating_accrue(a);
			user_info.clear_pending_reward(pool_info.pool_acc);
			StakingInfo::<T>::insert(&info_key, &user_info);
			// Lock the funds
			Self::ledger_accrue(&who, a);
			// Update pool info
			pool_info.total_stake = pool_info.total_stake.saturating_add(a);
			pool_info.free_stake = pool_info.free_stake.saturating_add(a);

			// we have new free stake now, try handle the waitting withdraw queue
			Self::try_process_withdraw_queue(&mut pool_info);

			MiningPools::<T>::insert(&pid, &pool_info);

			Self::deposit_event(Event::<T>::Deposit(pid, who, a));
			Ok(())
		}

		/// Withdraws some stake from a pool.
		///
		/// Note: there are two scenarios people may meet
		///
		/// - if the pool has free stake and and amount of the free stake greater than or equal to
		///    the withdraw amount (e.g. pool.free_stake >= amount), the withdraw would take effect
		//     immediately.
		/// - else the withdraw would be queued and delay untill there are enough free stake in the
		///    pool.
		#[pallet::weight(0)]
		pub fn withdraw(origin: OriginFor<T>, pid: u64, amount: BalanceOf<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let info_key = (pid.clone(), who.clone());
			let mut user_info =
				Self::staking_info(&info_key).ok_or(Error::<T>::StakeInfoNotFound)?;

			ensure!(
				amount > Zero::zero() && user_info.amount >= amount,
				Error::<T>::InvalidWithdrawAmount
			);

			let mut pool_info = Self::ensure_pool(pid)?;
			let now = <T as registry::Config>::UnixTime::now()
				.as_secs()
				.saturated_into::<u64>();

			// if withdraw_queue is not empty, means pool doesn't have free stake now, just add withdraw to queue
			if !pool_info.withdraw_queue.is_empty() {
				pool_info.withdraw_queue.push_back(WithdrawInfo {
					user: who.clone(),
					amount: amount,
					start_time: now,
				});
				Self::maybe_add_withdraw_queue(now, pool_info.pid);
			} else {
				Self::try_withdraw(&mut pool_info, &mut user_info, amount);
			}

			StakingInfo::<T>::insert(&info_key, &user_info);
			MiningPools::<T>::insert(&pid, &pool_info);

			Ok(())
		}

		/// Starts a miner on behalf of the stake pool
		///
		/// Requires:
		/// 1. The miner is bounded to the pool and is in Ready state
		/// 2. The remaining stake in the pool can cover the minimal stake requried
		#[pallet::weight(0)]
		pub fn start_mining(
			origin: OriginFor<T>,
			pid: u64,
			worker: WorkerPublicKey,
			stake: BalanceOf<T>,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let mut pool_info = Self::ensure_pool(pid)?;
			// origin must be owner of pool
			ensure!(pool_info.owner == owner, Error::<T>::UnauthorizedPoolOwner);
			// check free stake
			ensure!(pool_info.free_stake >= stake, Error::<T>::InsufficientStake);
			// check wheather we have add this worker
			ensure!(
				pool_info.workers.contains(&worker),
				Error::<T>::WorkerHasNotAdded
			);
			let miner: T::AccountId = pool_sub_account(pid, &worker);
			<mining::pallet::Pallet<T>>::set_deposit(&miner, stake);
			match <mining::pallet::Pallet<T>>::start_mining(miner.clone()) {
				Ok(()) => {
					pool_info.free_stake = pool_info.free_stake.saturating_sub(stake);
					MiningPools::<T>::insert(&pid, &pool_info);
				}
				_ => {
					// rollback
					<mining::pallet::Pallet<T>>::set_deposit(&miner, Zero::zero());
					return Err(Error::<T>::StartMiningCallFailed.into());
				}
			}

			Ok(())
		}

		/// Stops a miner on behalf of the stake pool
		/// Note: this would let miner enter coolingdown if everything is good
		///
		/// Requires:
		/// 1. There miner is bounded to the pool and is in a stoppable state
		#[pallet::weight(0)]
		pub fn stop_mining(
			origin: OriginFor<T>,
			pid: u64,
			worker: WorkerPublicKey,
		) -> DispatchResult {
			let owner = ensure_signed(origin)?;
			let pool_info = Self::ensure_pool(pid)?;
			// origin must be owner of pool
			ensure!(pool_info.owner == owner, Error::<T>::UnauthorizedPoolOwner);
			// check wheather we have add this worker
			ensure!(
				pool_info.workers.contains(&worker),
				Error::<T>::WorkerHasNotAdded
			);
			let miner: T::AccountId = pool_sub_account(pid, &worker);
			<mining::pallet::Pallet<T>>::stop_mining(miner)?;

			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		pub fn account_id() -> T::AccountId {
			STAKEPOOL_PALLETID.into_account()
		}

		/// Adds up the newly received reward to `pool_acc`
		fn handle_pool_new_reward(
			pool_info: &mut PoolInfo<T::AccountId, BalanceOf<T>>,
			rewards: BalanceOf<T>,
		) {
			if rewards > Zero::zero() && pool_info.total_stake > Zero::zero() {
				let commission = pool_info.payout_commission.unwrap_or_default() * rewards;
				pool_info.owner_reward.saturating_accrue(commission);
				pool_info.add_reward(rewards - commission);
			}
		}

		/// Tries to withdraw a specific amount from a pool.
		///
		/// The withdraw request would be delayed if the free stake is not enough, otherwise
		/// withdraw from the free deposit immediately.
		///
		/// WARNING:
		/// 1. The method assumes user pending reward is already cleared.
		/// 2. The updates are made in `pool_info` and `user_info`. It's up to the caller to
		///     persist the data.
		fn try_withdraw(
			pool_info: &mut PoolInfo<T::AccountId, BalanceOf<T>>,
			user_info: &mut UserStakeInfo<T::AccountId, BalanceOf<T>>,
			amount: BalanceOf<T>,
		) {
			// enough free stake, withdraw directly
			if pool_info.free_stake >= amount {
				pool_info.free_stake = pool_info.free_stake.saturating_sub(amount);
				pool_info.total_stake = pool_info.total_stake.saturating_sub(amount);
				user_info.amount = user_info.amount.saturating_sub(amount);
				Self::ledger_reduce(&user_info.user, amount);
				Self::deposit_event(Event::<T>::Withdraw(
					pool_info.pid,
					user_info.user.clone(),
					amount,
				));
			} else {
				let now = <T as registry::Config>::UnixTime::now()
					.as_secs()
					.saturated_into::<u64>();
				// all of the free_stake would be withdrew back to user
				let delta = pool_info.free_stake;
				let unwithdraw_amount = amount.saturating_sub(pool_info.free_stake);
				pool_info.total_stake = pool_info.total_stake.saturating_sub(delta);
				user_info.amount.saturating_reduce(delta);
				Self::ledger_reduce(&user_info.user, pool_info.free_stake);
				Self::deposit_event(Event::<T>::Withdraw(
					pool_info.pid,
					user_info.user.clone(),
					pool_info.free_stake,
				));
				pool_info.free_stake = Zero::zero();

				// case some locked asset has not been withdraw(unlock) to user, add it to withdraw queue.
				// when pool has free stake again, the withdraw would be handled
				pool_info.withdraw_queue.push_back(WithdrawInfo {
					user: user_info.user.clone(),
					amount: unwithdraw_amount,
					start_time: now,
				});
				Self::maybe_add_withdraw_queue(now, pool_info.pid);
			}
			// Update the pending reward after changing the staked amount
			user_info.clear_pending_reward(pool_info.pool_acc);
		}

		/// Tries to fulfill the withdraw queue with the newly freed stake
		fn try_process_withdraw_queue(pool_info: &mut PoolInfo<T::AccountId, BalanceOf<T>>) {
			while pool_info.free_stake > Zero::zero() {
				if let Some(mut withdraw) = pool_info.withdraw_queue.front().cloned() {
					// Must clear the pending reward before any stake change
					let info_key = (pool_info.pid.clone(), withdraw.user.clone());
					let mut user_info = Self::staking_info(&info_key).unwrap();
					pool_info.clear_user_pending_reward(&mut user_info);
					// Try to fulfill the withdraw requests as much as possible
					let delta = sp_std::cmp::min(pool_info.free_stake, withdraw.amount);
					pool_info.free_stake.saturating_reduce(delta);
					pool_info.total_stake.saturating_reduce(delta);
					withdraw.amount.saturating_reduce(delta);
					user_info.amount.saturating_reduce(delta);
					// Actually withdraw the funds
					Self::ledger_reduce(&user_info.user, delta);
					Self::deposit_event(Event::<T>::Withdraw(
						pool_info.pid,
						user_info.user.clone(),
						delta,
					));
					// Update the pending reward after changing the staked amount
					user_info.clear_pending_reward(pool_info.pool_acc);
					StakingInfo::<T>::insert(&info_key, &user_info);
					// Update if the withdraw is partially fulfilled, otherwise pop it out of the
					// queue
					if withdraw.amount == Zero::zero() {
						pool_info.withdraw_queue.pop_front();
					} else {
						*pool_info.withdraw_queue.front_mut().unwrap() = withdraw;
					}
				} else {
					break;
				}
			}
		}

		/// Updates a user's locked balance. Doesn't check the amount is less than the free amount!
		fn update_lock(who: &T::AccountId, amount: BalanceOf<T>) {
			if amount == Zero::zero() {
				<T as Config>::Currency::remove_lock(STAKING_ID, who);
			} else {
				<T as Config>::Currency::set_lock(STAKING_ID, who, amount, WithdrawReasons::all());
			}
		}

		/// Gets the pool record by `pid`. Returns error if not exist
		fn ensure_pool(pid: u64) -> Result<PoolInfo<T::AccountId, BalanceOf<T>>, Error<T>> {
			Self::mining_pools(&pid).ok_or(Error::<T>::PoolNotExist)
		}

		/// Adds the givin pool (`pid`) to the withdraw queue if not present
		fn maybe_add_withdraw_queue(start_time: u64, pid: u64) {
			let mut t = WithdrawTimestamps::<T>::get();
			if let Some(last_start_time) = t.back().cloned() {
				// the last_start_time == start_time means already have a withdraw request added early of this block,
				// last_start_time > start_time is impossible
				if last_start_time < start_time {
					t.push_back(start_time);
				}
			} else {
				// first time add withdraw pool
				t.push_back(start_time);
			}
			WithdrawTimestamps::<T>::put(&t);

			// push pool to the pool list, if the pool was added in this pool, means it has waiting withdraw request
			// in current block(if they have the same timestamp, we think they are in the same block)
			if WithdrawPools::<T>::contains_key(&start_time) {
				let mut pool_list = WithdrawPools::<T>::get(&start_time).unwrap();
				// if pool has already been added, ignore it
				if !pool_list.contains(&pid) {
					pool_list.push(pid);
					WithdrawPools::<T>::insert(&start_time, &pool_list);
				}
			} else {
				WithdrawPools::<T>::insert(&start_time, vec![pid]);
			}
		}
	}

	impl<T: Config> mining::OnReward for Pallet<T> {
		/// Called when gk send new payout information.
		/// Append specific miner's reward balance of current round,
		/// would be clear once pool was updated
		fn on_reward(settle: &Vec<SettleInfo>) {
			for info in settle {
				let pid = WorkerInPool::<T>::get(&info.pubkey)
					.expect("Mining workers must be in the pool; qed.");
				let mut pool_info = Self::ensure_pool(pid).expect("Stake pool must exist; qed.");

				Self::handle_pool_new_reward(&mut pool_info, info.payout.saturated_into());
				MiningPools::<T>::insert(&pid, &pool_info);
			}
		}
	}

	impl<T: Config> mining::OnCleanup<BalanceOf<T>> for Pallet<T>
	where
		T: mining::Config,
	{
		/// Called when worker was cleanuped
		/// After collingdown end, worker was cleanuped, whose deposit balance
		/// would be reset to zero
		fn on_cleanup(worker: &WorkerPublicKey, deposit_balance: BalanceOf<T>) {
			let pid =
				WorkerInPool::<T>::get(worker).expect("Mining workers must be in the pool; qed.");
			let mut pool_info = Self::ensure_pool(pid).expect("Stake pool must exist; qed.");

			// with the worker been cleaned, whose stake now are free
			pool_info.free_stake = pool_info.free_stake.saturating_add(deposit_balance);

			Self::try_process_withdraw_queue(&mut pool_info);
			MiningPools::<T>::insert(&pid, &pool_info);
		}
	}

	impl<T: Config> Ledger<T::AccountId, BalanceOf<T>> for Pallet<T> {
		fn ledger_accrue(who: &T::AccountId, amount: BalanceOf<T>) {
			let b: BalanceOf<T> = StakeLedger::<T>::get(who).unwrap_or_default();
			let new_b = b.saturating_add(amount);
			StakeLedger::<T>::insert(who, new_b);
			Self::update_lock(who, new_b);
		}

		fn ledger_reduce(who: &T::AccountId, amount: BalanceOf<T>) {
			let b: BalanceOf<T> = StakeLedger::<T>::get(who).unwrap_or_default();
			let new_b = b.saturating_sub(amount);
			StakeLedger::<T>::insert(who, new_b);
			Self::update_lock(who, new_b);
		}

		fn ledger_query(who: &T::AccountId) -> BalanceOf<T> {
			StakeLedger::<T>::get(who).unwrap_or_default()
		}
	}

	fn pool_sub_account<T>(pid: u64, pubkey: &WorkerPublicKey) -> T
	where
		T: Encode + Decode + Default,
	{
		let hash = crate::hashing::blake2_256(&(pid, pubkey).encode());
		// stake pool miner
		(b"spm/", hash)
			.using_encoded(|b| T::decode(&mut TrailingZeroInput::new(b)))
			.unwrap_or_default()
	}

	#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
	pub struct PoolInfo<AccountId: Default, Balance> {
		pid: u64,
		owner: AccountId,
		payout_commission: Option<Permill>,
		owner_reward: Balance,
		cap: Option<Balance>,
		pool_acc: Balance,
		total_stake: Balance,
		free_stake: Balance,
		workers: Vec<WorkerPublicKey>,
		withdraw_queue: VecDeque<WithdrawInfo<AccountId, Balance>>,
	}

	impl<AccountId, Balance> PoolInfo<AccountId, Balance>
	where
		AccountId: Default,
		Balance: sp_runtime::traits::AtLeast32BitUnsigned + Copy,
	{
		/// Clears the pending rewards of a user and move to `available_rewards` for claiming
		fn clear_user_pending_reward(&self, user_info: &mut UserStakeInfo<AccountId, Balance>) {
			let pending_reward = user_info.pending_reward(self.pool_acc);
			user_info
				.available_rewards
				.saturating_accrue(pending_reward);
			user_info.clear_pending_reward(self.pool_acc);
		}

		// Distributes additinoal rewards to the current share holders.
		//
		// Additional rewards contribute to the face value of the pool shares. The vaue of each
		// share effectively grows by (rewards / total_shares).
		fn add_reward(&mut self, rewards: Balance) {
			self.pool_acc
				.saturating_accrue(rewards * 10u32.pow(6).into() / self.total_stake);
		}
	}

	#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
	pub struct UserStakeInfo<AccountId: Default, Balance> {
		user: AccountId,
		amount: Balance,
		available_rewards: Balance,
		user_debt: Balance,
	}

	#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
	pub struct WithdrawInfo<AccountId: Default, Balance> {
		user: AccountId,
		amount: Balance,
		start_time: u64,
	}

	pub struct EnsurePool<T>(sp_std::marker::PhantomData<T>);
	impl<T: Config> EnsureOrigin<T::Origin> for EnsurePool<T> {
		type Success = T::AccountId;
		fn try_origin(o: T::Origin) -> Result<Self::Success, T::Origin> {
			let pool_id = STAKEPOOL_PALLETID.into_account();
			o.into().and_then(|o| match o {
				frame_system::RawOrigin::Signed(who) if who == pool_id => Ok(pool_id),
				r => Err(T::Origin::from(r)),
			})
		}
	}

	impl<AccountId, Balance> UserStakeInfo<AccountId, Balance>
	where
		AccountId: Default,
		Balance: sp_runtime::traits::AtLeast32BitUnsigned + Copy,
	{
		/// Calculates the pending reward this user holds
		///
		/// - `acc_per_share`: accumulated reward per share
		fn pending_reward(&self, acc_per_share: Balance) -> Balance {
			self.amount * acc_per_share / 1_000_000u32.into() - self.user_debt
		}

		/// Resets the `user_debt` to remove all the pending rewards
		fn clear_pending_reward(&mut self, acc_per_share: Balance) {
			self.user_debt = self.amount * acc_per_share / 1_000_000u32.into();
		}
	}

	#[cfg(test)]
	mod test {
		use assert_matches::assert_matches;
		use frame_support::{assert_noop, assert_ok};
		use hex_literal::hex;
		use sp_runtime::AccountId32;

		use super::*;
		use crate::mock::{
			ecdh_pubkey,
			new_test_ext,
			set_block_1,
			take_events,
			worker_pubkey,
			Balance,
			Balances,
			Event as TestEvent,
			Origin,
			// Pallets
			PhalaRegistry,
			PhalaStakePool,
			Test,
			// Constants
			DOLLARS,
		};

		#[test]
		fn test_pool_subaccount() {
			let sub_account: AccountId32 =
				pool_sub_account(1, &WorkerPublicKey::from_raw([0u8; 32]));
			let expected = AccountId32::new(hex!(
				"73706d2f02ab4d74c86ec3b3997a4fadf33e55e8279650c8539ea67e053c02dc"
			));
			assert_eq!(sub_account, expected, "Incorrect sub account");
		}

		#[test]
		fn test_create() {
			// Check this fixed: <https://github.com/Phala-Network/phala-blockchain/issues/285>
			new_test_ext().execute_with(|| {
				set_block_1();
				assert_ok!(PhalaStakePool::create(Origin::signed(1)));
				assert_ok!(PhalaStakePool::create(Origin::signed(1)));
				PhalaStakePool::on_finalize(1);
				assert_matches!(
					take_events().as_slice(),
					[
						TestEvent::PhalaStakePool(Event::PoolCreated(1, 0)),
						TestEvent::PhalaStakePool(Event::PoolCreated(1, 1)),
					]
				);
				assert_eq!(
					MiningPools::<Test>::get(0),
					Some(PoolInfo {
						pid: 0,
						owner: 1,
						payout_commission: None,
						owner_reward: 0,
						cap: None,
						pool_acc: 0,
						total_stake: 0,
						free_stake: 0,
						workers: Vec::new(),
						withdraw_queue: VecDeque::new(),
					})
				);
				assert_eq!(PoolCount::<Test>::get(), 2);
			});
		}

		#[test]
		fn test_add_worker() {
			new_test_ext().execute_with(|| {
				set_block_1();
				let worker1 = worker_pubkey(1);
				let worker2 = worker_pubkey(2);

				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker1.clone(),
					ecdh_pubkey(1),
					Some(1)
				));

				// Create a pool (pid = 0)
				assert_ok!(PhalaStakePool::create(Origin::signed(1)));
				// Bad inputs
				assert_noop!(
					PhalaStakePool::add_worker(Origin::signed(1), 1, worker2.clone()),
					Error::<Test>::WorkerNotRegistered
				);
				assert_noop!(
					PhalaStakePool::add_worker(Origin::signed(2), 0, worker1.clone()),
					Error::<Test>::UnauthorizedOperator
				);
				assert_noop!(
					PhalaStakePool::add_worker(Origin::signed(1), 0, worker1.clone()),
					Error::<Test>::BenchmarkMissing
				);
				// Add benchmark and retry
				PhalaRegistry::internal_set_benchmark(&worker1, Some(1));
				assert_ok!(PhalaStakePool::add_worker(
					Origin::signed(1),
					0,
					worker1.clone()
				));
				// Other bad cases
				assert_noop!(
					PhalaStakePool::add_worker(Origin::signed(1), 100, worker1.clone()),
					Error::<Test>::PoolNotExist
				);
				// Bind one worker to antoher pool (pid = 1)
				assert_ok!(PhalaStakePool::create(Origin::signed(1)));
				assert_noop!(
					PhalaStakePool::add_worker(Origin::signed(1), 1, worker1.clone()),
					Error::<Test>::MinerBindingCallFailed
				);
			});
		}

		#[test]
		fn test_pool_cap() {
			new_test_ext().execute_with(|| {
				set_block_1();
				let worker1 = worker_pubkey(1);
				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker1.clone(),
					ecdh_pubkey(1),
					Some(1)
				));

				assert_ok!(PhalaStakePool::create(Origin::signed(1))); // pid = 0
				assert_eq!(PhalaStakePool::mining_pools(0).unwrap().cap, None);
				// Pool existence
				assert_noop!(
					PhalaStakePool::set_cap(Origin::signed(2), 100, 1),
					Error::<Test>::PoolNotExist,
				);
				// Owner only
				assert_noop!(
					PhalaStakePool::set_cap(Origin::signed(2), 0, 1),
					Error::<Test>::UnauthorizedPoolOwner,
				);
				// Cap to 1000 PHA
				assert_ok!(PhalaStakePool::set_cap(
					Origin::signed(1),
					0,
					1000 * DOLLARS
				));
				assert_eq!(
					PhalaStakePool::mining_pools(0).unwrap().cap,
					Some(1000 * DOLLARS)
				);
				// Check cap shouldn't be less than the current stake
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 100 * DOLLARS));
				assert_noop!(
					PhalaStakePool::set_cap(Origin::signed(1), 0, 99 * DOLLARS),
					Error::<Test>::InvalidCapacity,
				);
				// Stake to the cap
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 900 * DOLLARS));
				// Exceed the cap
				assert_noop!(
					PhalaStakePool::deposit(Origin::signed(1), 0, 900 * DOLLARS),
					Error::<Test>::StakeExceedCapacity,
				);
			});
		}

		#[test]
		fn test_stake() {
			new_test_ext().execute_with(|| {
				set_block_1();
				let worker1 = worker_pubkey(1);
				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker1.clone(),
					ecdh_pubkey(1),
					Some(1)
				));

				assert_ok!(PhalaStakePool::create(Origin::signed(1))); // pid = 0
				assert_ok!(PhalaStakePool::create(Origin::signed(2))); // pid = 1

				// Stake normally
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 1 * DOLLARS));
				assert_ok!(PhalaStakePool::deposit(Origin::signed(2), 0, 10 * DOLLARS));
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 1, 100 * DOLLARS));
				assert_ok!(PhalaStakePool::deposit(
					Origin::signed(2),
					1,
					1000 * DOLLARS
				));
				// Check total stake
				assert_eq!(
					PhalaStakePool::mining_pools(0).unwrap().total_stake,
					11 * DOLLARS
				);
				assert_eq!(
					PhalaStakePool::mining_pools(1).unwrap().total_stake,
					1100 * DOLLARS
				);
				// Check total locks
				assert_eq!(PhalaStakePool::stake_ledger(1), Some(101 * DOLLARS));
				assert_eq!(PhalaStakePool::stake_ledger(2), Some(1010 * DOLLARS));
				assert_eq!(Balances::locks(1), vec![the_lock(101 * DOLLARS)]);
				assert_eq!(Balances::locks(2), vec![the_lock(1010 * DOLLARS)]);

				// Pool existence
				assert_noop!(
					PhalaStakePool::deposit(Origin::signed(1), 100, 1 * DOLLARS),
					Error::<Test>::PoolNotExist
				);
				// Dust deposit
				assert_noop!(
					PhalaStakePool::deposit(Origin::signed(1), 0, 1),
					Error::<Test>::LessThanMinDeposit
				);
				// Stake more than account1 has
				assert_noop!(
					PhalaStakePool::deposit(Origin::signed(1), 0, Balances::free_balance(1) + 1,),
					Error::<Test>::InsufficientBalance,
				);
			});
		}

		#[test]
		fn test_reward_management() {
			use crate::mining::pallet::OnReward;
			new_test_ext().execute_with(|| {
				set_block_1();
				setup_workers(1);
				setup_pool_with_workers(1, &[1]); // pid = 0

				// Check stake before receiving any rewards
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 100 * DOLLARS));
				assert_ok!(PhalaStakePool::deposit(Origin::signed(2), 0, 400 * DOLLARS));
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				assert_eq!(pool.pool_acc, 0);
				assert_eq!(pool.total_stake, 500 * DOLLARS);

				// Mined 500 PHA
				PhalaStakePool::on_reward(&vec![SettleInfo {
					pubkey: worker_pubkey(1),
					v: 1,
					payout: 500 * DOLLARS,
				}]);
				// Should result in 100, 400 PHA pending reward for staker1 & 2
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.pool_acc, 1_000_000);
				assert_eq!(staker1.pending_reward(pool.pool_acc), 100 * DOLLARS);
				assert_eq!(staker2.pending_reward(pool.pool_acc), 400 * DOLLARS);

				// Staker1 claims 100 PHA rewrad, left 100 debt & no pending reward
				let _ = take_events();
				assert_ok!(PhalaStakePool::claim_reward(Origin::signed(1), 0, 1));
				assert_eq!(
					take_events().as_slice(),
					[TestEvent::PhalaStakePool(Event::WithdrawRewards(
						0,
						1,
						100 * DOLLARS
					))]
				);
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				assert_eq!(pool.pool_acc, 1_000_000, "pool_acc shouldn't change");
				assert_eq!(staker1.user_debt, 100 * DOLLARS);
				assert_eq!(staker1.pending_reward(pool.pool_acc), 0);

				// Mined 500 PHA
				PhalaStakePool::on_reward(&vec![SettleInfo {
					pubkey: worker_pubkey(1),
					v: 1,
					payout: 500 * DOLLARS,
				}]);
				// Should result in 100, 800 PHA pending reward for staker1 & 2
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.pool_acc, 2_000_000);
				assert_eq!(staker1.pending_reward(pool.pool_acc), 100 * DOLLARS);
				assert_eq!(staker2.pending_reward(pool.pool_acc), 800 * DOLLARS);

				// Staker2 claims 800 PHA rewrad, left 800 debt
				let _ = take_events();
				assert_ok!(PhalaStakePool::claim_reward(Origin::signed(2), 0, 2));
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(staker2.user_debt, 800 * DOLLARS);

				// Staker1 deposit another 300 PHA (now 50:50), causing a passive reward settlement
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 300 * DOLLARS));
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				assert_eq!(staker1.amount, 400 * DOLLARS);
				assert_eq!(staker1.user_debt, 800 * DOLLARS);

				// Mined 800 PHA
				PhalaStakePool::on_reward(&vec![SettleInfo {
					pubkey: worker_pubkey(1),
					v: 1,
					payout: 800 * DOLLARS,
				}]);
				assert_ok!(PhalaStakePool::claim_reward(Origin::signed(1), 0, 1));
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.pool_acc, 3_000_000);
				assert_eq!(staker1.pending_reward(pool.pool_acc), 0);
				assert_eq!(staker2.pending_reward(pool.pool_acc), 400 * DOLLARS);

				// Staker1 withdraw all
				let _ = take_events();
				assert_ok!(PhalaStakePool::withdraw(
					Origin::signed(1),
					0,
					400 * DOLLARS
				));
				assert_eq!(
					take_events().as_slice(),
					[TestEvent::PhalaStakePool(Event::Withdraw(
						0,
						1,
						400 * DOLLARS
					))]
				);
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(staker1.amount, 0);
				assert_eq!(staker1.user_debt, 0);
				assert_eq!(staker2.amount, 400 * DOLLARS);
			});
		}

		#[test]
		fn test_withdraw() {
			use crate::mining::pallet::OnCleanup;
			new_test_ext().execute_with(|| {
				set_block_1();
				setup_workers(2);
				setup_pool_with_workers(1, &[1, 2]); // pid = 0

				// Stake 1000 PHA, and start two miners with 400 & 100 PHA as stake
				assert_ok!(PhalaStakePool::deposit(
					Origin::signed(2),
					0,
					1000 * DOLLARS
				));
				assert_ok!(PhalaStakePool::start_mining(
					Origin::signed(1),
					0,
					worker_pubkey(1),
					400 * DOLLARS
				));
				assert_ok!(PhalaStakePool::start_mining(
					Origin::signed(1),
					0,
					worker_pubkey(2),
					100 * DOLLARS
				));
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(staker2.amount, 1000 * DOLLARS);
				assert_eq!(Balances::locks(2), vec![the_lock(1000 * DOLLARS)]);
				// Immediate withdraw 499 PHA from the free stake
				assert_ok!(PhalaStakePool::withdraw(
					Origin::signed(2),
					0,
					499 * DOLLARS
				));
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.free_stake, 1 * DOLLARS);
				assert_eq!(pool.total_stake, 501 * DOLLARS);
				assert_eq!(staker2.amount, 501 * DOLLARS);
				assert_eq!(Balances::locks(2), vec![the_lock(501 * DOLLARS)]);
				// Withdraw 2 PHA will only fulfill 1 PHA from the free stake, leaving 1 PHA in the
				// withdraw queue
				assert_ok!(PhalaStakePool::withdraw(Origin::signed(2), 0, 2 * DOLLARS));
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.free_stake, 0);
				assert_eq!(pool.total_stake, 500 * DOLLARS);
				assert_eq!(staker2.amount, 500 * DOLLARS);
				assert_eq!(Balances::locks(2), vec![the_lock(500 * DOLLARS)]);
				// Check the queue
				assert_eq!(
					pool.withdraw_queue,
					vec![WithdrawInfo {
						user: 2,
						amount: 1 * DOLLARS,
						start_time: 0
					}]
				);
				let ts_queue = PhalaStakePool::withdraw_timestamps();
				assert_eq!(ts_queue.len(), 1);
				assert_eq!(
					PhalaStakePool::withdraw_pools(ts_queue.front().unwrap()),
					Some(vec![0])
				);

				// Deposit 1 PHA to trigger instant withdraw, fulfilling the withdraw request.
				// Then staker1 has 1PHA in stake, and staker2 only has 499 PHA in stake.
				let _ = take_events();
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 1 * DOLLARS));
				assert_eq!(
					take_events().as_slice(),
					[
						TestEvent::PhalaStakePool(Event::Withdraw(0, 2, 1 * DOLLARS)),
						TestEvent::PhalaStakePool(Event::Deposit(0, 1, 1 * DOLLARS))
					]
				);
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.free_stake, 0);
				assert_eq!(pool.total_stake, 500 * DOLLARS);
				assert_eq!(pool.withdraw_queue.is_empty(), true);
				assert_eq!(staker1.amount, 1 * DOLLARS);
				assert_eq!(staker2.amount, 499 * DOLLARS);
				assert_eq!(Balances::locks(2), vec![the_lock(499 * DOLLARS)]);
				// Withdraw 199 PHA, queued, and then wait for force clear
				assert_ok!(PhalaStakePool::withdraw(
					Origin::signed(2),
					0,
					199 * DOLLARS
				));
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(
					pool.withdraw_queue,
					vec![WithdrawInfo {
						user: 2,
						amount: 199 * DOLLARS,
						start_time: 0
					}]
				);
				assert_eq!(staker2.amount, 499 * DOLLARS);
				// Trigger a force clear by `on_cleanup()`, releasing 100 PHA stake to partially
				// fulfill staker2's withdraw request
				let _ = take_events();
				PhalaStakePool::on_cleanup(&worker_pubkey(2), 100 * DOLLARS);
				assert_eq!(
					take_events().as_slice(),
					[TestEvent::PhalaStakePool(Event::Withdraw(
						0,
						2,
						100 * DOLLARS
					)),]
				);
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.total_stake, 400 * DOLLARS);
				assert_eq!(pool.free_stake, 0);
				assert_eq!(staker1.amount, 1 * DOLLARS);
				assert_eq!(staker2.amount, 399 * DOLLARS);
				// Trigger another force clear, releasing all the remaining 400 PHA stake,
				// fulfilling staker2's request.
				// Then all 301 PHA becomes free, and there are 1 & 300 PHA loced by the stakers.
				let _ = take_events();
				PhalaStakePool::on_cleanup(&worker_pubkey(1), 400 * DOLLARS);
				assert_eq!(
					take_events().as_slice(),
					[TestEvent::PhalaStakePool(Event::Withdraw(
						0,
						2,
						99 * DOLLARS
					)),]
				);
				let pool = PhalaStakePool::mining_pools(0).unwrap();
				let staker1 = PhalaStakePool::staking_info((0, 1)).unwrap();
				let staker2 = PhalaStakePool::staking_info((0, 2)).unwrap();
				assert_eq!(pool.total_stake, 301 * DOLLARS);
				assert_eq!(pool.free_stake, 301 * DOLLARS);
				assert_eq!(staker1.amount, 1 * DOLLARS);
				assert_eq!(staker2.amount, 300 * DOLLARS);
				assert_eq!(Balances::locks(1), vec![the_lock(1 * DOLLARS)]);
				assert_eq!(Balances::locks(2), vec![the_lock(300 * DOLLARS)]);

				// TODO: handle slash at on_cleanup()
			});
		}

		#[test]
		fn test_full_procedure() {
			new_test_ext().execute_with(|| {
				set_block_1();
				let worker1 = worker_pubkey(1);
				let worker2 = worker_pubkey(2);
				let worker3 = worker_pubkey(3);
				// Register workers
				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker1.clone(),
					ecdh_pubkey(1),
					Some(1)
				));
				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker2.clone(),
					ecdh_pubkey(2),
					Some(1)
				));
				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker3.clone(),
					ecdh_pubkey(3),
					Some(1)
				));
				PhalaRegistry::internal_set_benchmark(&worker1, Some(1));
				PhalaRegistry::internal_set_benchmark(&worker2, Some(1));
				PhalaRegistry::internal_set_benchmark(&worker3, Some(1));

				// Create a pool (pid = 0)
				assert_ok!(PhalaStakePool::create(Origin::signed(1)));
				assert_ok!(PhalaStakePool::set_payout_pref(
					Origin::signed(1),
					0,
					Permill::from_percent(50)
				));
				assert_ok!(PhalaStakePool::add_worker(
					Origin::signed(1),
					0,
					worker1.clone()
				));
				assert_ok!(PhalaStakePool::add_worker(
					Origin::signed(1),
					0,
					worker2.clone()
				));
				// Create a pool (pid = 1)
				assert_ok!(PhalaStakePool::create(Origin::signed(1)));
				assert_ok!(PhalaStakePool::add_worker(
					Origin::signed(1),
					1,
					worker3.clone()
				));

				assert_ok!(PhalaStakePool::set_cap(Origin::signed(1), 0, 300 * DOLLARS));
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 0, 100 * DOLLARS));
				assert_eq!(StakeLedger::<Test>::get(1).unwrap(), 100 * DOLLARS);
				assert_ok!(PhalaStakePool::deposit(Origin::signed(1), 1, 300 * DOLLARS));
				assert_eq!(StakeLedger::<Test>::get(1).unwrap(), 400 * DOLLARS);
				assert_eq!(
					MiningPools::<Test>::get(0).unwrap().total_stake,
					100 * DOLLARS
				);
				assert_eq!(
					StakingInfo::<Test>::get(&(0, 1)).unwrap().amount,
					100 * DOLLARS
				);

				assert_ok!(PhalaStakePool::deposit(Origin::signed(2), 0, 200 * DOLLARS));
				assert_eq!(
					MiningPools::<Test>::get(0).unwrap().total_stake,
					300 * DOLLARS
				);
				assert_eq!(
					StakingInfo::<Test>::get(&(0, 2)).unwrap().amount,
					200 * DOLLARS
				);

				assert_noop!(
					PhalaStakePool::deposit(Origin::signed(1), 0, 100 * DOLLARS),
					Error::<Test>::StakeExceedCapacity
				);

				assert_ok!(PhalaStakePool::start_mining(
					Origin::signed(1),
					0,
					worker1.clone(),
					100 * DOLLARS
				));
				assert_ok!(PhalaStakePool::start_mining(
					Origin::signed(1),
					0,
					worker2.clone(),
					100 * DOLLARS
				));
				// Withdraw free funds
				assert_ok!(PhalaStakePool::withdraw(
					Origin::signed(1),
					0,
					100 * DOLLARS
				));
				assert_eq!(StakeLedger::<Test>::get(1).unwrap(), 300 * DOLLARS);

				// TODO: check balance
				// TODO: check queued withdraw
				//   - withdraw 100 PHA
				//   - stop a worker
				//   - wait CD, withdraw succeeded
				//   - withdraw another 100 PHA
				//   - wait 3d, force stop
				//   - wait 7d, withdraw succeeded
			});
		}

		fn the_lock(amount: Balance) -> pallet_balances::BalanceLock<Balance> {
			pallet_balances::BalanceLock {
				id: STAKING_ID,
				amount,
				reasons: pallet_balances::Reasons::All,
			}
		}

		/// Sets up `n` workers starting from 1, registered and benchmarked.
		fn setup_workers(n: u8) {
			for i in 1..=n {
				let worker = worker_pubkey(i);
				assert_ok!(PhalaRegistry::force_register_worker(
					Origin::root(),
					worker.clone(),
					ecdh_pubkey(1),
					Some(1)
				));
				PhalaRegistry::internal_set_benchmark(&worker, Some(1));
			}
		}

		/// Sets up a stakepool with the given workers added.
		///
		/// Returns the pool id.
		fn setup_pool_with_workers(owner: u64, workers: &[u8]) -> u64 {
			let pid = PhalaStakePool::pool_count();
			assert_ok!(PhalaStakePool::create(Origin::signed(owner)));
			for id in workers {
				assert_ok!(PhalaStakePool::add_worker(
					Origin::signed(owner),
					pid,
					worker_pubkey(*id),
				));
			}
			pid
		}
	}
}
