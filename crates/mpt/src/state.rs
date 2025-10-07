use bumpalo::Bump;
use reth_trie::TrieAccount;
use revm::database::BundleState;
use revm_primitives::{keccak256, map::DefaultHashBuilder, HashMap, B256};

<<<<<<< HEAD
use itertools::Itertools;
use reth_primitives::Account;
use reth_revm::db::{AccountStatus, BundleAccount};
use reth_trie::{prefix_set::PrefixSetMut, Nibbles};
use revm_primitives::{
    hash_map, keccak256, map::DefaultHashBuilder, Address, HashMap, HashSet, B256, U256,
};
use std::borrow::Cow;
=======
use crate::{Error, Mpt};
>>>>>>> d15946928217eeb3ccfb6d6718587840db12f28e

/// Serialized Ethereum state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EthereumStateBytes {
    pub state_trie: (usize, bytes::Bytes),
    pub storage_tries: Vec<(B256, usize, bytes::Bytes)>,
}

#[derive(Debug, Clone)]
pub struct EthereumState {
    pub state_trie: Mpt<'static>,
    pub storage_tries: HashMap<B256, Mpt<'static>>,
    pub bump: &'static Bump,
}

<<<<<<< HEAD
        let mut accounts =
            HashMap::with_capacity_and_hasher(hashed.len(), DefaultHashBuilder::default());
        let mut storages =
            HashMap::with_capacity_and_hasher(hashed.len(), DefaultHashBuilder::default());
        for (address, (account, storage)) in hashed {
            accounts.insert(address, account);
            storages.insert(address, storage);
        }
        Self { accounts, storages }
    }

    /// Construct [`HashedPostState`] from a single [`HashedStorage`].
    pub fn from_hashed_storage(hashed_address: B256, storage: HashedStorage) -> Self {
        Self {
            accounts: HashMap::default(),
            storages: [(hashed_address, storage)].into_iter().collect(),
        }
    }

    /// Set account entries on hashed state.
    pub fn with_accounts(
        mut self,
        accounts: impl IntoIterator<Item = (B256, Option<Account>)>,
    ) -> Self {
        self.accounts = HashMap::from_iter(accounts);
        self
    }

    /// Set storage entries on hashed state.
    pub fn with_storages(
        mut self,
        storages: impl IntoIterator<Item = (B256, HashedStorage)>,
    ) -> Self {
        self.storages = HashMap::from_iter(storages);
        self
    }

    /// Extend this hashed post state with contents of another.
    /// Entries in the second hashed post state take precedence.
    pub fn extend(&mut self, other: Self) {
        self.extend_inner(Cow::Owned(other));
    }

    /// Extend this hashed post state with contents of another.
    /// Entries in the second hashed post state take precedence.
    ///
    /// Slightly less efficient than [`Self::extend`], but preferred to `extend(other.clone())`.
    pub fn extend_ref(&mut self, other: &Self) {
        self.extend_inner(Cow::Borrowed(other));
    }

    fn extend_inner(&mut self, other: Cow<'_, Self>) {
        self.accounts.extend(other.accounts.iter().map(|(&k, &v)| (k, v)));

        self.storages.reserve(other.storages.len());
        match other {
            Cow::Borrowed(other) => {
                self.extend_storages(other.storages.iter().map(|(k, v)| (*k, Cow::Borrowed(v))))
            }
            Cow::Owned(other) => {
                self.extend_storages(other.storages.into_iter().map(|(k, v)| (k, Cow::Owned(v))))
            }
=======
impl EthereumState {
    pub fn new() -> Self {
        let bump = Box::leak(Box::new(Bump::new()));
        Self {
            state_trie: Mpt::new(bump),
            storage_tries: HashMap::with_capacity_and_hasher(1, DefaultHashBuilder::default()),
            bump,
>>>>>>> d15946928217eeb3ccfb6d6718587840db12f28e
        }
    }

    pub fn update_from_bundle_state(&mut self, bundle_state: &BundleState) -> Result<(), Error> {
        for (address, account) in &bundle_state.state {
            let hashed_address = keccak256(address);

            if let Some(info) = &account.info {
                let storage_trie =
                    self.storage_tries.entry(hashed_address).or_insert(Mpt::new(self.bump));

                if account.status.was_destroyed() {
                    *storage_trie = Mpt::new(self.bump);
                }

                for (slot, value) in &account.storage {
                    let hashed_slot = keccak256(slot.to_be_bytes::<32>());
                    if value.present_value.is_zero() {
                        storage_trie.delete(hashed_slot.as_slice())?;
                    } else {
                        storage_trie.insert_rlp(hashed_slot.as_slice(), value.present_value)?;
                    }
                }
                let storage_root = storage_trie.hash();
                let state_account = TrieAccount {
                    nonce: info.nonce,
                    balance: info.balance,
                    storage_root,
                    code_hash: info.code_hash,
                };
                self.state_trie.insert_rlp(hashed_address.as_slice(), state_account)?;
            } else {
                self.state_trie.delete(hashed_address.as_slice()).unwrap();
                self.storage_tries.remove(&hashed_address);
            }
        }

        Ok(())
    }

    #[cfg(feature = "host")]
    pub fn encode_to_state_bytes(&self) -> EthereumStateBytes {
        let state_num_nodes = self.state_trie.num_nodes();
        let state_bytes = bytes::Bytes::from(self.state_trie.encode_trie());
        let mut storage_bytes: Vec<_> = self
            .storage_tries
            .iter()
            .map(|(addr, trie)| (*addr, trie.num_nodes(), bytes::Bytes::from(trie.encode_trie())))
            .collect();
        storage_bytes.sort_by_key(|(addr, _, _)| *addr);

        EthereumStateBytes {
            state_trie: (state_num_nodes, state_bytes),
            storage_tries: storage_bytes,
        }
    }
}

impl Default for EthereumState {
    fn default() -> Self {
        Self::new()
    }
}
