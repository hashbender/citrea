use borsh::BorshDeserialize;
use sov_modules_api::BlobReaderTrait;
use sov_rollup_interface::da::{DaDataLightClient, DaNamespace, DaVerifier};
use sov_rollup_interface::zk::{
    BatchProofCircuitOutput, BatchProofInfo, LightClientCircuitInput, LightClientCircuitOutput,
    ZkvmGuest,
};
use sov_rollup_interface::Network;

use crate::utils::{collect_unchained_outputs, recursive_match_state_roots};

#[derive(Debug)]
pub enum LightClientVerificationError {
    DaTxsCouldntBeVerified,
    HeaderChainVerificationFailed,
    InvalidPreviousLightClientProof,
}

pub fn run_circuit<DaV: DaVerifier, G: ZkvmGuest>(
    da_verifier: DaV,
    input: LightClientCircuitInput<DaV::Spec>,
    l2_genesis_root: [u8; 32],
    batch_proof_method_id: [u32; 8],
    batch_prover_da_public_key: &[u8],
    network: Network,
) -> Result<LightClientCircuitOutput, LightClientVerificationError> {
    // Extract previous light client proof output
    let previous_light_client_proof_output =
        if let Some(journal) = input.previous_light_client_proof_journal {
            let prev_output = G::verify_and_extract_output::<LightClientCircuitOutput>(
                &journal,
                &input.light_client_proof_method_id.into(),
            )
            .map_err(|_| LightClientVerificationError::InvalidPreviousLightClientProof)?;
            // Ensure method IDs match
            assert_eq!(
                input.light_client_proof_method_id,
                prev_output.light_client_proof_method_id,
            );
            Some(prev_output)
        } else {
            None
        };

    let new_da_state = da_verifier
        .verify_header_chain(
            previous_light_client_proof_output
                .as_ref()
                .map(|output| &output.latest_da_state),
            &input.da_block_header,
            network,
        )
        .map_err(|_| LightClientVerificationError::HeaderChainVerificationFailed)?;

    // Verify data from da
    da_verifier
        .verify_transactions(
            &input.da_block_header,
            input.da_data.as_slice(),
            input.inclusion_proof,
            input.completeness_proof,
            DaNamespace::ToLightClientProver,
        )
        .map_err(|_| LightClientVerificationError::DaTxsCouldntBeVerified)?;

    // Mapping from initial state root to final state root and last L2 height
    let mut initial_to_final = std::collections::BTreeMap::<[u8; 32], ([u8; 32], u64)>::new();

    let (mut last_state_root, mut last_l2_height) =
        previous_light_client_proof_output.as_ref().map_or_else(
            || {
                // if no previous proof, we start from genesis state root
                (l2_genesis_root, 0)
            },
            |prev_journal| (prev_journal.state_root, prev_journal.last_l2_height),
        );

    // If we have a previous light client proof, check they can be chained
    // If not, skip for now
    if let Some(previous_output) = &previous_light_client_proof_output {
        for unchained_info in previous_output.unchained_batch_proofs_info.iter() {
            // Add them directly as they are the ones that could not be matched
            initial_to_final.insert(
                unchained_info.initial_state_root,
                (
                    unchained_info.final_state_root,
                    unchained_info.last_l2_height,
                ),
            );
        }
    }
    // TODO: Test for multiple assumptions to see if the env::verify function does automatic matching between the journal and the assumption or do we need to verify them in order?
    // https://github.com/chainwayxyz/citrea/issues/1401
    // Parse the batch proof da data
    for blob in input.da_data {
        if blob.sender().as_ref() == batch_prover_da_public_key {
            let data = DaDataLightClient::try_from_slice(blob.verified_data());

            if let Ok(data) = data {
                match data {
                    DaDataLightClient::Complete(proof) => {
                        let journal =
                            G::extract_raw_output(&proof).expect("DaData proofs must be valid");
                        // TODO: select output version based on the spec
                        let batch_proof_output: BatchProofCircuitOutput<DaV::Spec, [u8; 32]> =
                            match G::verify_and_extract_output(
                                &journal,
                                &batch_proof_method_id.into(),
                            ) {
                                Ok(output) => output,
                                Err(_) => continue,
                            };

                        // Do not add if last l2 height is smaller or equal to previous output
                        // This is to defend against replay attacks, for example if somehow there is the script of batch proof 1 we do not need to go through it again
                        if batch_proof_output.last_l2_height <= last_l2_height {
                            continue;
                        }

                        recursive_match_state_roots(
                            &mut initial_to_final,
                            &BatchProofInfo::new(
                                batch_proof_output.initial_state_root,
                                batch_proof_output.final_state_root,
                                batch_proof_output.last_l2_height,
                            ),
                        );
                    }
                    DaDataLightClient::Aggregate(_) => todo!(),
                    DaDataLightClient::Chunk(_) => todo!(),
                }
            }
        }
    }

    // Do recursive matching for previous state root
    recursive_match_state_roots(
        &mut initial_to_final,
        &BatchProofInfo::new(last_state_root, last_state_root, last_l2_height),
    );

    // Now only thing left is the state update if exists and others are unchained
    if let Some((final_root, last_l2)) = initial_to_final.remove(&last_state_root) {
        last_l2_height = last_l2;
        last_state_root = final_root;
    }

    // Collect unchained outputs
    let unchained_outputs = collect_unchained_outputs(&initial_to_final, last_l2_height);

    Ok(LightClientCircuitOutput {
        state_root: last_state_root,
        light_client_proof_method_id: input.light_client_proof_method_id,
        latest_da_state: new_da_state,
        unchained_batch_proofs_info: unchained_outputs,
        last_l2_height,
    })
}
