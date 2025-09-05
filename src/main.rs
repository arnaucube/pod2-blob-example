use alloy::{
    consensus::{SidecarBuilder, SimpleCoder},
    eips::eip4844::DATA_GAS_PER_BLOB,
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use std::{
    fs,
    fs::File,
    io::{BufReader, Read, Write},
    time::Instant,
};

use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover},
    frontend::{MainPodBuilder, Operation},
    middleware::{containers::Set, Params},
};

// ethereum private key to use for the tx
const PRIV_KEY: String = "PLACE THE ETHEREUM PRIV KEY HERE";
// ethereum node rpc url
const RPC_URL: String = "PLACE THE RPC URL HERE";

// returns a MainPod, example adapted from pod2/examples/main_pod_points.rs
pub fn compute_pod_proof() -> Result<pod2::frontend::MainPod> {
    let params = Params {
        max_input_pods: 0,
        ..Default::default()
    };

    let mut builder = MainPodBuilder::new(&params, &DEFAULT_VD_SET);
    let set_entries = [1, 2, 3].into_iter().map(|n| n.into()).collect();
    let set = Set::new(10, set_entries)?;

    builder.pub_op(Operation::set_contains(set, 1))?;

    let prover = Prover {};
    let pod = builder.prove(&prover).unwrap();
    Ok(pod)
}

#[tokio::main]
async fn main() -> Result<()> {
    // PART 1: generate a pod2 proof, and wrap it into one level recursive proof
    // in order to shrink its size

    // use the compute_pod_proof to obtain an example pod2 proof
    println!("start to generate a pod2 proof");
    let start = Instant::now();
    let pod = compute_pod_proof()?;
    println!(
        "[TIME] generate pod & compute pod proof took: {:?}",
        start.elapsed()
    );
    // generate new plonky2 proof from POD's proof. This is 1 extra recursion in
    // order to shrink the proof size, together with removing extra custom gates
    let start = Instant::now();
    let (verifier_data, common_circuit_data, proof_with_pis) = pod2_onchain::prove_pod(pod)?;
    println!("[TIME] plonky2 (wrapper) proof took: {:?}", start.elapsed());

    // get the compressed proof, which we will send inside a blob
    let compressed_proof = proof_with_pis.compress(
        &verifier_data.verifier_only.circuit_digest,
        &common_circuit_data.common,
    )?;
    let compressed_proof_bytes = compressed_proof.to_bytes();
    // store it in a file just in case we want to check it later
    let mut file = fs::File::create("proof_with_public_inputs.bin")?;
    file.write_all(&compressed_proof_bytes)?;
    dbg!(&compressed_proof_bytes.len());

    // alternatively, instead of generating the pod2 proof, we can load a
    // previously stored proof with public inputs from the file
    /*
    let file = File::open("./proof_with_public_inputs.bin")?;
    let mut reader = BufReader::new(file);
    let mut compressed_proof_bytes = Vec::new();
    reader.read_to_end(&mut compressed_proof_bytes)?;
    */
    println!("size of proof_with_pis: {}", compressed_proof_bytes.len());

    // PART 2: send the pod2 proof into a tx blob
    let signer: PrivateKeySigner = PRIV_KEY.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect(RPC_URL)
        .await?;

    let latest_block = provider.get_block_number().await?;
    println!("Latest block number: {latest_block}");

    let alice = signer.address();
    let bob = Address::from([0x42; 20]);
    dbg!(&alice);
    dbg!(&bob);

    let sidecar: SidecarBuilder<SimpleCoder> = SidecarBuilder::from_slice(&compressed_proof_bytes);
    let sidecar = sidecar.build()?;

    let tx = TransactionRequest::default()
        // 'from' field is filled by signer's first address (Alice in our case)
        .with_to(bob)
        .with_blob_sidecar(sidecar);

    let pending_tx = provider.send_transaction(tx).await?;

    println!("Pending transaction... tx hash: {}", pending_tx.tx_hash());

    let receipt = pending_tx.get_receipt().await?;

    println!(
        "Transaction included in block {}",
        receipt.block_number.expect("Failed to get block number")
    );

    assert_eq!(receipt.from, alice);
    assert_eq!(receipt.to, Some(bob));
    assert_eq!(
        receipt
            .blob_gas_used
            .expect("Expected to be EIP-4844 transaction"),
        DATA_GAS_PER_BLOB
    );

    Ok(())
}
