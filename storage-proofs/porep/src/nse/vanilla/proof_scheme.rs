use anyhow::{ensure, Context};
use storage_proofs_core::{
    error::Result,
    hasher::Hasher,
    merkle::{MerkleProofTrait, MerkleTreeTrait},
    proof::ProofScheme,
};

use super::{
    hash_comm_r, ChallengeRequirements, Challenges, NarrowStackedExpander, Parent, PrivateInputs,
    Proof, PublicInputs, PublicParams, SetupParams,
};

impl<'a, Tree: 'static + MerkleTreeTrait, G: 'static + Hasher> ProofScheme<'a>
    for NarrowStackedExpander<'a, Tree, G>
{
    type PublicParams = PublicParams<Tree>;
    type SetupParams = SetupParams;
    type PublicInputs = PublicInputs<<Tree::Hasher as Hasher>::Domain, <G as Hasher>::Domain>;
    type PrivateInputs = PrivateInputs<Tree, G>;
    type Proof = Vec<Proof<Tree, G>>;
    type Requirements = ChallengeRequirements;

    fn setup(sp: &Self::SetupParams) -> Result<Self::PublicParams> {
        Ok(sp.clone().into())
    }

    fn prove<'b>(
        pub_params: &'b Self::PublicParams,
        pub_inputs: &'b Self::PublicInputs,
        priv_inputs: &'b Self::PrivateInputs,
    ) -> Result<Self::Proof> {
        let proofs = Self::prove_all_partitions(pub_params, pub_inputs, priv_inputs, 1)?;
        let k = pub_inputs.k.unwrap_or_default();

        // Because partition proofs require a common setup, the general ProofScheme implementation,
        // which makes use of `ProofScheme::prove` cannot be used here. Instead, we need to prove all
        // partitions in one pass, as implemented by `prove_all_partitions` below.
        assert!(
            k < 1,
            "It is a programmer error to call NarrowStackedExpander::prove with more than one partition."
        );

        Ok(proofs[k].to_owned())
    }

    fn prove_all_partitions<'b>(
        pub_params: &'b Self::PublicParams,
        pub_inputs: &'b Self::PublicInputs,
        priv_inputs: &'b Self::PrivateInputs,
        partition_count: usize,
    ) -> Result<Vec<Self::Proof>> {
        ensure!(partition_count > 0, "partitions must not be 0");

        let config = &pub_params.config;
        let challenges = Challenges::new(
            config,
            pub_params.num_challenges_window,
            &pub_inputs.replica_id,
            pub_inputs.seed,
        );

        assert_eq!(
            partition_count, 1,
            "multiple partitions are not implemented yet"
        );

        let mut proofs = Vec::new();

        let butterfly_parents = super::ButterflyGraph::from(config);
        let exp_parents = super::ExpanderGraph::from(config);

        for challenge in challenges {
            // Data Inclusion Proof
            let data_proof = priv_inputs
                .t_aux
                .tree_d
                .gen_proof(challenge.node)
                .context("failed to create data proof")?;

            // Layer Inclusion Proof
            let layer_tree = if challenge.layer == config.num_layers() {
                &priv_inputs.t_aux.tree_replica
            } else {
                &priv_inputs.t_aux.layers[challenge.layer - 1]
            };
            let levels = priv_inputs.t_aux.tree_config_levels;
            let layer_proof = layer_tree
                .gen_cached_proof(challenge.node, levels)
                .context("failed to create layer proof")?;

            // Labeling Proofs
            let parents: Vec<Parent> = if config.is_layer_expander(challenge.layer) {
                exp_parents
                    .expanded_parents(challenge.node as u32)
                    .flatten()
                    .collect()
            } else {
                butterfly_parents
                    .parents(challenge.node as u32, challenge.layer as u32)
                    .collect()
            };

            let parents_proofs = parents
                .iter()
                .map(|parent| {
                    layer_tree
                        .gen_cached_proof(*parent as usize, levels)
                        .context("failed to create parent proof")
                })
                .collect::<Result<_>>()?;

            // roots for the layers
            let mut comm_layers = priv_inputs.p_aux.comm_layers.clone();
            comm_layers.push(priv_inputs.p_aux.comm_replica);

            proofs.push(Proof::new(
                data_proof,
                layer_proof,
                parents,
                parents_proofs,
                comm_layers,
            ));
        }

        Ok(vec![proofs])
    }

    fn verify_all_partitions(
        pub_params: &Self::PublicParams,
        pub_inputs: &Self::PublicInputs,
        partition_proofs: &[Self::Proof],
    ) -> Result<bool> {
        let config = &pub_params.config;
        let is_valid = partition_proofs.iter().enumerate().all(|(k, proofs)| {
            let pub_inputs = Self::with_partition(pub_inputs.clone(), Some(k));
            let tau = pub_inputs.tau.as_ref().expect("missing tau");

            let challenges = Challenges::new(
                config,
                pub_params.num_challenges_window,
                &pub_inputs.replica_id,
                pub_inputs.seed,
            );

            for (proof, challenge) in proofs.iter().zip(challenges) {
                // verify comm_r
                let last = proof.comm_layers.len() - 1;
                let comm_r: <Tree::Hasher as Hasher>::Domain =
                    hash_comm_r(&proof.comm_layers[..last], proof.comm_layers[last]).into();
                check_eq!(comm_r, tau.comm_r);

                // verify data inclusion
                check!(proof.data_proof.verify());
                check_eq!(proof.data_proof.root(), tau.comm_d);

                // verify layer inclusion
                check!(proof.layer_proof.verify());
                check_eq!(
                    proof.layer_proof.root(),
                    proof.comm_layers[challenge.layer - 1]
                );

                // Verify labeling
                // TODO: verify number of proofs
                for parents_proof in &proof.parents_proofs {
                    check!(parents_proof.verify());
                    check_eq!(parents_proof.root(), proof.comm_layers[challenge.layer - 1]);
                }

                // TODO: actual labeling
            }

            true
        });

        Ok(is_valid)
    }

    fn with_partition(mut pub_in: Self::PublicInputs, k: Option<usize>) -> Self::PublicInputs {
        pub_in.k = k;
        pub_in
    }

    fn satisfies_requirements(
        _public_params: &PublicParams<Tree>,
        _requirements: &ChallengeRequirements,
        _partitions: usize,
    ) -> bool {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use generic_array::typenum::{Unsigned, U0, U2, U8};
    use merkletree::store::StoreConfig;
    use rand::{Rng, SeedableRng};
    use rand_xorshift::XorShiftRng;
    use storage_proofs_core::cache_key::CacheKey;
    use storage_proofs_core::{
        hasher::{Domain, PoseidonHasher, Sha256Hasher},
        merkle::LCTree,
        proof::ProofScheme,
    };

    use super::super::{Config, TemporaryAuxCache};
    use crate::PoRep;

    #[test]
    fn test_prove_verify() {
        type Tree = LCTree<PoseidonHasher, U8, U8, U0>;

        let rng = &mut XorShiftRng::from_seed(crate::TEST_SEED);
        let replica_id = <PoseidonHasher as Hasher>::Domain::random(rng);
        let config = Config {
            k: 8,
            num_nodes_window: 64,
            degree_expander: 12,
            degree_butterfly: 8,
            num_expander_layers: 3,
            num_butterfly_layers: 3,
            sector_size: 64 * 32 * 8,
        };

        let data: Vec<u8> = (0..config.num_nodes_sector())
            .flat_map(|_| {
                let v = <PoseidonHasher as Hasher>::Domain::random(rng);
                v.into_bytes()
            })
            .collect();

        // create a copy, so we can compare roundtrips
        let mut data_copy = data.clone();

        let sp = SetupParams {
            config: config.clone(),
            num_challenges_window: 2,
        };

        let pp = NarrowStackedExpander::<Tree, Sha256Hasher>::setup(&sp).expect("setup failed");

        // MT for original data is always named tree-d, and it will be
        // referenced later in the process as such.
        let cache_dir = tempfile::tempdir().unwrap();
        let store_config = StoreConfig::new(
            cache_dir.path(),
            CacheKey::CommDTree.to_string(),
            StoreConfig::default_cached_above_base_layer(config.num_nodes_sector(), U2::to_usize()),
        );

        // Generate a replica path.
        let temp_dir = tempdir::TempDir::new("test-extract-all").unwrap();
        let temp_path = temp_dir.path();
        let replica_path = temp_path.join("replica-path");

        let (tau, (p_aux, t_aux)) = NarrowStackedExpander::<Tree, Sha256Hasher>::replicate(
            &pp,
            &replica_id,
            (&mut data_copy[..]).into(),
            None,
            store_config.clone(),
            replica_path.clone(),
        )
        .expect("replication failed");
        assert_ne!(data, data_copy);

        let seed = rng.gen();

        let pub_inputs = PublicInputs::<
            <<Tree as MerkleTreeTrait>::Hasher as Hasher>::Domain,
            <Sha256Hasher as Hasher>::Domain,
        > {
            replica_id,
            seed,
            tau: Some(tau),
            k: None,
        };

        // Store a copy of the t_aux for later resource deletion.
        let t_aux_orig = t_aux.clone();

        // Convert TemporaryAux to TemporaryAuxCache, which instantiates all
        // elements based on the configs stored in TemporaryAux.
        let t_aux = TemporaryAuxCache::<Tree, Sha256Hasher>::new(&config, &t_aux, replica_path)
            .expect("failed to restore contents of t_aux");

        let priv_inputs = PrivateInputs { p_aux, t_aux };
        let partitions = 1;

        let all_partition_proofs =
            &NarrowStackedExpander::<Tree, Sha256Hasher>::prove_all_partitions(
                &pp,
                &pub_inputs,
                &priv_inputs,
                partitions,
            )
            .expect("failed to generate partition proofs");

        let proofs_are_valid = NarrowStackedExpander::<Tree, Sha256Hasher>::verify_all_partitions(
            &pp,
            &pub_inputs,
            all_partition_proofs,
        )
        .expect("failed to verify partition proofs");

        // Discard cached MTs that are no longer needed.
        t_aux_orig.clear_temp().expect("t_aux delete failed");

        assert!(proofs_are_valid);
    }
}