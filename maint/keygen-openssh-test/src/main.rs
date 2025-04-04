mod args;

use args::{Args, KeyType};
use tor_llcrypto::pk::ed25519::Ed25519PublicKey as _;
use tor_llcrypto::util::rng::RngCompat;

use std::fs;

use clap::Parser;

use ssh_key::private::{DsaKeypair, Ed25519Keypair, Ed25519PrivateKey, OpaqueKeypair};
use ssh_key::public::{DsaPublicKey, Ed25519PublicKey, OpaquePublicKey};
use ssh_key::{self, Algorithm, AlgorithmName, PrivateKey, PublicKey};
use tor_basic_utils::test_rng::testing_rng;
use tor_llcrypto::pk::{curve25519, ed25519};

/// A helper for creating a ([`PrivateKey`], [`PublicKey`]) pair.
macro_rules! make_openssh_key {
    ($kind:tt, $args:expr, $keypair:expr, $public:expr) => {{
        let comment = $args.comment.clone().unwrap_or("test-key".into());
        let openssh_key = ssh_key::public::PublicKey::new(
            ssh_key::public::KeyData::$kind($public),
            comment.clone(),
        );
        let openssh_private = ssh_key::private::PrivateKey::new(
            ssh_key::private::KeypairData::$kind($keypair),
            comment,
        )
        .unwrap();

        (openssh_private, openssh_key)
    }};
}

/// Generate an ed25519-expanded ssh key.
fn generate_expanded_ed25519(args: &Args) -> (PrivateKey, PublicKey) {
    let algo = args
        .algorithm
        .clone()
        .unwrap_or("ed25519-expanded@spec.torproject.org".into());
    let algorithm_name = AlgorithmName::new(algo).unwrap();

    let mut rng = testing_rng();
    let ed25519_kp = ed25519::Keypair::generate(&mut rng);
    let expanded_kp: ed25519::ExpandedKeypair = (&ed25519_kp).into();
    let ssh_public = OpaquePublicKey::new(
        expanded_kp.public().to_bytes().to_vec(),
        Algorithm::Other(algorithm_name),
    );
    let keypair = OpaqueKeypair::new(
        expanded_kp.to_secret_key_bytes().to_vec(),
        ssh_public.clone(),
    );

    make_openssh_key!(Other, args, keypair, ssh_public)
}

/// Generate an ed25519-expanded ssh key.
fn generate_ed25519(args: &Args) -> (PrivateKey, PublicKey) {
    let mut rng = testing_rng();
    let ed25519_kp = ed25519::Keypair::generate(&mut rng);
    let public_key_bytes: [u8; 32] = ed25519_kp
        .public_key()
        .to_bytes()
        .to_vec()
        .try_into()
        .unwrap();
    let public = Ed25519PublicKey(public_key_bytes);
    let secret_key_bytes: [u8; 32] = ed25519_kp.to_bytes().to_vec().try_into().unwrap();
    let private = Ed25519PrivateKey::from_bytes(&secret_key_bytes);
    let keypair = Ed25519Keypair { public, private };

    make_openssh_key!(Ed25519, args, keypair, public)
}

/// Generate a DSA ssh key.
fn generate_dsa(args: &Args) -> (PrivateKey, PublicKey) {
    let mut rng = RngCompat::new(testing_rng());
    let keypair = DsaKeypair::random(&mut rng).unwrap();
    let public = DsaPublicKey::from(&keypair);

    make_openssh_key!(Dsa, args, keypair, public)
}

/// Generate an x25519 ssh key.
fn generate_x25519(args: &Args) -> (PrivateKey, PublicKey) {
    let rng = testing_rng();
    let x25519_sk = curve25519::StaticSecret::random_from_rng(rng);
    let x25519_pk = curve25519::PublicKey::from(&x25519_sk);

    let algo = args
        .algorithm
        .clone()
        .unwrap_or("x25519@spec.torproject.org".into());
    let algorithm_name = AlgorithmName::new(algo).unwrap();

    let public = OpaquePublicKey::new(
        x25519_pk.to_bytes().to_vec(),
        Algorithm::Other(algorithm_name),
    );
    let keypair = OpaqueKeypair::new(x25519_sk.to_bytes().to_vec(), public.clone());

    make_openssh_key!(Other, args, keypair, public)
}

fn main() {
    let args = Args::parse();

    // Figure out if we're generating a public key, a private key, or both.
    let (gen_pub, gen_priv) = match (args.public, args.private) {
        (false, false) => {
            // If neither --public nor --private is specified, generate both.
            (true, true)
        }
        (gen_pub, gen_priv) => (gen_pub, gen_priv),
    };

    let (openssh_private, openssh_public) = match &args.key_type {
        KeyType::ExpandedEd25519 => generate_expanded_ed25519(&args),
        KeyType::Ed25519 => generate_ed25519(&args),
        KeyType::Dsa => generate_dsa(&args),
        KeyType::X25519 => generate_x25519(&args),
    };

    let public = openssh_public.to_openssh().unwrap();
    let private = openssh_private
        .to_openssh(ssh_key::LineEnding::LF)
        .unwrap()
        .to_string();

    let pub_file = format!("{}.public", args.name);
    let priv_file = format!("{}.private", args.name);

    if gen_pub {
        fs::write(&pub_file, public).unwrap();
        println!("created {pub_file}");
    }

    if gen_priv {
        fs::write(&priv_file, private).unwrap();
        println!("created {priv_file}");
    }
}
