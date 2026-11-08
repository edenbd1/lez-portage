//! pol-measure: build the jobs, check them natively against the Bedrock
//! node's own code, run them in r0vm.
//!
//!   pol-measure gen VECTOR.json OUT_DIR
//!       VECTOR.json: vectors/bedrock/testnet-lez-36401.json (31 public-testnet
//!       headers). Writes OUT_DIR/{pol-ark,pol-ark-1,pol-bn,pol-bn-1,notes-ark,notes-bn}.job
//!       and OUT_DIR/vectors.json (the fresh proof, its inputs, the testnet
//!       headers' proofs and what the node's verifier says of each).
//!   pol-measure run GUEST.bin JOB [--prove]
//!       Executes (or proves, then verifies the receipt) and prints one JSON
//!       line: session cycles, segments, the guest's steps and verdicts, wall
//!       time.

use std::{path::Path, str::FromStr, time::Instant};

use ark_ff::{BigInteger, PrimeField};
use ark_serialize::CanonicalSerialize;
use lb_groth16::{Fr, Groth16VerificationKey, Groth16VerificationKeyJsonDeser};
use lb_pol::{LotteryConstants, PolChainInputsData, PolVerifierInput, PolWalletInputsData, PolWitnessInputsData};
use lb_poseidon2::{Digest, Poseidon2Bn254Hasher};
use num_bigint::BigUint;
use pol_measure_core::{ark, bn, p2, root_from, BnVk, FrLe, Job, NoteOp, PolCase, Report};
use serde_json::{json, Value};

fn fr_le(f: &Fr) -> FrLe {
    ark::fr_bytes(f)
}

fn big(s: &str) -> Fr {
    BigUint::from_str(s).unwrap().into()
}

fn write_job(dir: &Path, name: &str, job: &Job) {
    let b = borsh::to_vec(job).unwrap();
    std::fs::write(dir.join(format!("{name}.job")), &b).unwrap();
    eprintln!("{name}.job: {} bytes", b.len());
}

/// The node's own test of the PoL circuit (`zk/proofs/pol/src/lib.rs`,
/// `test_full_flow`): its chain and wallet data, proven with `lb_pol::prove`
/// (the v0.5.7 witness generator and proving key, rapidsnark).
fn fresh_proof() -> (lb_pol::PoLProof, PolVerifierInput) {
    let (lottery_0, lottery_1) =
        LotteryConstants::new(lb_utils::math::NonNegativeRatio::new(1, 10.try_into().unwrap())).compute_lottery_values(5000);
    let chain = PolChainInputsData {
        slot_number: 135,
        epoch_nonce: Fr::from(510u64),
        lottery_0,
        lottery_1,
        aged_root: big("16524395010779500501330992017298834046369952285388149958144954382059764408785"),
        latest_root: big("20834259014818668135947196643658285248830563263522051603918285129600429840356"),
        leader_pk: (BigUint::from(123_456u32).into(), BigUint::from(654_321u32).into()),
    };
    let aged_path = [
        "19205963370485772287624660148655123421630250475080881163694783019817455316910",
        "842632995957934154490829566755258332089735588762651714354950816371199202948",
        "11945865010023840792771634133332309536300021618163415115524743891577396784532",
        "5678144262683681210286056464240523249603444097705978250116845087015739879001",
        "4543130200496840387394591809962125944112292744388860937179706396873037283532",
        "447945249172309706147685180974075378036414184792819934794462245951881132491",
        "10984937925416446764477546426399361164722183078917024384765615640208309773886",
        "8332301048884746072225650742152218830832205382305719788443252112373451390008",
        "7717740854062780302371479099548413295076442606971953244362182813969746810145",
        "5870767669864048222619695329761075412560256425762658309939091044024798841980",
        "1270394570818312305068877518098833848392252257574032052368322099978490528993",
        "4421806191049823072310631508282545862661318788050986720831912352661110024704",
        "9543220744965548298592940239013154329526126759818391282972665149911444460380",
        "11832520211727528097498428496026205325545655289771652718959221711574756323845",
        "8261267697784487706399178572428457281508840530987009187944836942560051990845",
        "7259688284711792346738578773128765401934616369296796752701532318548818675656",
        "20410766059815893992135193121776619328345084011501351449727415160723164671894",
        "19409422102345646102663034240923589054723801239665575001424649313393572448797",
        "2026478415805648536252469432565155067383923909238350287385051253116157491081",
        "21072644004334270034357972934702520036515345635901155899590483624455566877570",
        "12706352973548089350218643189492694819549197991366076961997024182862825891572",
        "20651842981778843765422134357942811568755148902817286832703401476578926932159",
        "19097976423347244117755030245169411514551470052335850544780149827855495070628",
        "10254517281922373005124180252373170574492353445403856807250684829458178341748",
        "4828393086141028784920329285136836834160589347467272527124976967197826969967",
        "7290848553264685940106614387259971050759824319302446980477663653328347224981",
        "13568116581247600056306081066087207159958446188434534967837954912335560567528",
        "19673474827150301713419447780509555813516015784591535595280509514259612835348",
        "14016712741485891526208755187452390849270211823944034841285668141997053149398",
        "20984444386080019249642854750185394191547415630952463570781495995361188304235",
        "20716527055704913432992003250600357730017309147012913323969779133930909800072",
        "14787709838677865776105327831675542255717739581860994014618609782788824576885",
    ]
    .map(big);
    let latest_path = [
        "11709948088963960065647371537879293701565786386460016885512089239291870378840",
        "2120901090324525908662474041962168618570906102338290469215218624292046179330",
        "10497564299466181298229172082520996337033686748361035747293006440896232093004",
        "3425260074297640180775893568433815888755716019399506534916465904285025646859",
        "4583167269494370304032520085783972434215767473666242416208450810280024023142",
        "12731396449267797964642841435803473321701941536173578364548242959504253386489",
        "20876509042032154075839888677829108280893892531171309282531399417530514655652",
        "19667250381457364671121915728778240317672011326702060802555641657494453986631",
        "20151748203430641227294477190013185951512285826753694689236792791802228236409",
        "5296503914834159504532891530796715207930043126952673371378640399578370383358",
        "15204004164053373401239131144856853301410255568638848470242417306501189833079",
        "8433140840890890569751171491136473504934304014919055457596233419295898933678",
        "10455135882248302013748933783036206895663823903064929584400314540485565598947",
        "6618514926108954914636276316487586150653225326091109610916623668888506574898",
        "8472865127566208631416954061285083236000644409874339646609286596717483042203",
        "5163990834740603021957302291480735541147224500392813452221842181941789763448",
        "21866665746348412966861756211614206576737908673885956395292046005817224269106",
        "17218628924715383670149819392265573714457473274338484988014313554129536528070",
        "13526373749917680118104762439292019605872872007383862555414186681846583503283",
        "14211156411104090766360751999017487063048079319839607558904054235600169155160",
        "11718317735557902839618433161716283335065018548585948157910312975459461756987",
        "16829743339703043966744159514162467606976583091056897908269705566108589164163",
        "6011104885108544056095362574030165839534992991860687552312346356400158313655",
        "1562596472155451649509003097294568270949518819233462734625356240396837358704",
        "12754783870620687365721563814820635276715427311805413156076695645151610773481",
        "8290249361628686401202188242691875538991713007249069067400817079004557471604",
        "14976734201356719360023861818781680042905193161258424609193595233171839570457",
        "10727243010131564083538628277491833886599073630784978208903889116861519604518",
        "14768425552740035340703385928185193645196603681658457264517638180469299892809",
        "15946835915681372862173515288501920089675395784378131928249939063433663089244",
        "18200637942483578342976888834386216643273358976477818116888783794758722873887",
        "15874955117141602197688150287249257989589905293755369275305245316881887378284",
    ]
    .map(big);
    let wallet = PolWalletInputsData {
        note_value: 50,
        transaction_hash: big("13842549853504288180344790242994935826382638205146161564439947279024930588677"),
        output_number: 8313,
        aged_path,
        aged_selectors: [
            false, true, true, false, true, true, false, true, false, false, true, false, true, false, false, true,
            false, true, true, true, true, false, true, true, true, false, false, false, false, true, true, true,
        ],
        latest_path,
        latest_selectors: [
            false, true, false, false, true, true, false, false, false, false, true, false, false, true, true, true,
            true, true, true, false, false, true, false, true, false, false, true, true, true, true, true, false,
        ],
        secret_key: big("7897218687652577456193628084912129251352759708723100638805247670738317482408"),
    };
    let t = Instant::now();
    let out = lb_pol::prove(PolWitnessInputsData::from_chain_and_wallet_data(chain, wallet).into()).expect("a PoL proof");
    eprintln!("lb_pol::prove: {:.2?}", t.elapsed());
    out
}

fn ark_g1(p: &ark_bn254::G1Affine) -> [[u8; 32]; 2] {
    let be = |f: &ark_bn254::Fq| -> [u8; 32] { f.into_bigint().to_bytes_be().try_into().unwrap() };
    [be(&p.x), be(&p.y)]
}

fn ark_g2(p: &ark_bn254::G2Affine) -> [[u8; 32]; 4] {
    let be = |f: &ark_bn254::Fq| -> [u8; 32] { f.into_bigint().to_bytes_be().try_into().unwrap() };
    [be(&p.x.c0), be(&p.x.c1), be(&p.y.c0), be(&p.y.c1)]
}

fn vk_json() -> Groth16VerificationKey {
    let j: Groth16VerificationKeyJsonDeser = serde_json::from_slice(lbc_pol_sys::artifacts::VERIFICATION_KEY).unwrap();
    j.try_into().unwrap()
}

/// A note id as the node computes it (`core/src/mantle/ledger.rs`, `Utxo::id`):
/// digest([NOTE_ID_V1, op_id, output_index, value, pk]).
fn note_preimage(i: u64) -> Vec<Fr> {
    let note_id_v1 = lb_groth16::fr_from_bytes(b"NOTE_ID_V1").unwrap();
    let op_id = Fr::from_le_bytes_mod_order(&lb_poseidon2::Poseidon2Bn254Hasher::digest(&[Fr::from(i)]).into_bigint().to_bytes_le());
    vec![note_id_v1, op_id, Fr::from(i % 3), Fr::from(1_000 + i), Fr::from(7 * i + 11)]
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key(Fr);
impl AsRef<Fr> for Key {
    fn as_ref(&self) -> &Fr {
        &self.0
    }
}
type Tree = lb_utxotree::UtxoTree<Key, (), Poseidon2Bn254Hasher>;

fn path_witness(tree: &Tree, k: &Key) -> (u32, Vec<FrLe>) {
    let path = tree.path(k).expect("a path");
    let mut pos = 0u32;
    let mut sib = Vec::new();
    for (i, n) in path.iter().enumerate() {
        match n {
            lb_utxotree::MerkleNode::Left(s) => {
                pos |= 1 << i;
                sib.push(fr_le(s));
            }
            lb_utxotree::MerkleNode::Right(s) => sib.push(fr_le(s)),
        }
    }
    (pos, sib)
}

fn gen(vector: &Path, out: &Path) {
    std::fs::create_dir_all(out).unwrap();

    // The key: lbc-pol-sys v0.5.7's verification_key.json, as POL_VK loads it.
    let ark_vk: ark_groth16::VerifyingKey<ark_bn254::Bn254> = vk_json().into();
    let pvk = vk_json().into_prepared();
    let mut vk_bytes = Vec::new();
    ark_vk.serialize_uncompressed(&mut vk_bytes).unwrap();
    let mut pvk_bytes = Vec::new();
    pvk.as_ref().serialize_uncompressed(&mut pvk_bytes).unwrap();
    assert_eq!(ark_vk.gamma_abc_g1.len(), 10, "nine public inputs");
    let bn_vk = BnVk {
        alpha_g1: ark_g1(&ark_vk.alpha_g1),
        beta_g2: ark_g2(&ark_vk.beta_g2),
        gamma_g2: ark_g2(&ark_vk.gamma_g2),
        delta_g2: ark_g2(&ark_vk.delta_g2),
        gamma_abc_g1: ark_vk.gamma_abc_g1.iter().map(ark_g1).collect(),
    };
    let bn_key = bn::key(&bn_vk);

    // Every case: the node's verdict, then both measured implementations
    // natively, which must agree with it.
    let mut cases = Vec::new();
    let mut doc = Vec::new();
    let mut add = |label: String, proof: [u8; 128], inputs: [Fr; 9], node: bool, note: Value| {
        let a = ark::verify(pvk.as_ref(), &proof, &inputs);
        let bn_inputs: Vec<_> = inputs.iter().map(|f| bn::fr(&fr_le(f))).collect();
        let b = bn::verify(&bn_key, &proof, &bn_inputs);
        assert_eq!((a, b), (node, node), "{label}: ark {a}, bn {b}, node {node}");
        if let Some(ark_groth16::Proof { a: pa, b: pb, c: pc }) = ark::expand(&proof) {
            let (qa, qb, qc) = bn::expand(&proof).expect("bn decompresses what ark does");
            let g1 = |p: &ark_bn254::G1Affine, q: &substrate_bn::G1| {
                let c = ark_g1(p);
                bn::g1(&c) == *q
            };
            let g2 = |p: &ark_bn254::G2Affine, q: &substrate_bn::G2| bn::g2(&ark_g2(p)) == *q;
            assert!(g1(&pa, &qa) && g2(&pb, &qb) && g1(&pc, &qc), "{label}: the same points");
        }
        doc.push(json!({"label": label, "proof": hex::encode(proof), "inputs": inputs.iter().map(|f| f.to_string()).collect::<Vec<_>>(), "node_verify": node, "note": note}));
        cases.push(PolCase { label, proof: proof.to_vec(), inputs: inputs.iter().map(fr_le).collect(), expect: node });
    };

    // 1. A fresh proof: valid, under the deployed key.
    let (proof, inputs) = fresh_proof();
    let node = lb_pol::verify(&proof, &inputs).expect("verifies");
    assert!(node, "the node accepts its own fresh proof");
    add("fresh-valid".into(), proof.to_bytes(), inputs.to_inputs(), node, json!("lb_pol::prove on the node's test_full_flow inputs; lb_pol::verify true"));
    // ... and the same proof with one input changed (the latest root), which
    // the node refuses: the cost of a rejection.
    let mut bad = inputs.to_inputs();
    bad[6] += Fr::from(1u64);
    let node_bad = lb_groth16::groth16_verify(&pvk, &lb_groth16::Groth16Proof::try_from(&proof).unwrap(), &bad).unwrap();
    assert!(!node_bad);
    add("fresh-wrong-latest-root".into(), proof.to_bytes(), bad, false, json!("the fresh proof with latest_root + 1; groth16_verify false"));

    // 2. The public-testnet headers: their proofs are real, their public
    // inputs partly unknowable here (epoch nonce, lottery values, aged and
    // latest roots are ledger state), so the node's own verifier refuses them
    // with zeros there; the cycle count is what matters.
    let v: Value = serde_json::from_slice(&std::fs::read(vector).unwrap()).unwrap();
    let mut headers = vec![v["anchor_block"]["header"].as_str().unwrap().to_string()];
    headers.extend(v["anchor_block"]["descendants"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()));
    let mut slots = Vec::new();
    for (i, h) in headers.iter().enumerate() {
        let r = hex::decode(h).unwrap();
        assert_eq!(r.len(), 296);
        let slot = u64::from_le_bytes(r[0..8].try_into().unwrap());
        slots.push(slot);
        let proof: [u8; 128] = r[72..200].try_into().unwrap();
        let entropy = lb_groth16::fr_from_bytes(&r[200..232]).expect("a canonical entropy");
        let pk = (lb_groth16::fr_from_bytes(&r[232..248]).unwrap(), lb_groth16::fr_from_bytes(&r[248..264]).unwrap());
        let z = Fr::from(0u64);
        let pi = PolVerifierInput::new(entropy, slot, z, z, z, z, z, pk);
        let node = lb_pol::verify(&lb_pol::PoLProof::from_bytes(&proof), &pi).unwrap_or(false);
        add(format!("testnet-{i:02}-slot-{slot}"), proof, pi.to_inputs(), node, json!("public-testnet header; epoch nonce, lottery and both roots unknown here (zero)"));
        if i == 0 {
            // The same header with full-width stand-ins for the five ledger
            // values (the fresh proof's lottery values; nonce and roots
            // Poseidon2 outputs, 254-bit like the real ones): what a real
            // check costs, since scalar multiplication cost follows the bits.
            let f = inputs.to_inputs();
            let h = |x: u64| Poseidon2Bn254Hasher::digest(&[Fr::from(x)]);
            let pi = PolVerifierInput::new(entropy, slot, h(1), h(2), h(3), f[3], f[4], pk);
            let node = lb_pol::verify(&lb_pol::PoLProof::from_bytes(&proof), &pi).unwrap_or(false);
            add(format!("testnet-{i:02}-full-width-stand-ins"), proof, pi.to_inputs(), node, json!("public-testnet header 0 with full-width stand-ins for nonce and roots and the fresh proof's lottery values; the representative cost of one real check"));
        }
    }

    // 3. Note-set updates on the node's tree: 1,000 notes, then two
    // insertions and two removals, each with its witness from the tree.
    let mut tree = Tree::new();
    for i in 0..1_000u64 {
        tree = tree.insert(Key(Poseidon2Bn254Hasher::digest(&note_preimage(i))), ()).0;
    }
    let mut ops = Vec::new();
    for (insert, i) in [(true, 5_000u64), (false, 17), (true, 5_001), (false, 640)] {
        let pre = note_preimage(i);
        let id = Key(Poseidon2Bn254Hasher::digest(&pre));
        let old_root = tree.root();
        let (pos, sib, new_tree) = if insert {
            let (t, pos) = tree.insert(id.clone(), ());
            let (p, s) = path_witness(&t, &id);
            assert_eq!(p as usize, pos);
            (p, s, t)
        } else {
            let (p, s) = path_witness(&tree, &id);
            let (t, ()) = tree.remove(&id).unwrap();
            (p, s, t)
        };
        let new_root = new_tree.root();
        // the witness reproduces the node's tree, both ways
        let compress = |a: Fr, b: Fr| <Poseidon2Bn254Hasher as Digest>::compress(&[a, b]);
        let s: Vec<Fr> = sib.iter().map(ark::fr).collect();
        let (from, to) = if insert { (Fr::from(0u64), id.0) } else { (id.0, Fr::from(0u64)) };
        assert_eq!(root_from(from, pos, &s, &compress), old_root);
        assert_eq!(root_from(to, pos, &s, &compress), new_root);
        ops.push(NoteOp {
            insert,
            preimage: pre.iter().map(fr_le).collect(),
            note_id: fr_le(&id.0),
            position: pos,
            siblings: sib,
            old_root: fr_le(&old_root),
            new_root: fr_le(&new_root),
        });
        tree = new_tree;
    }
    // Poseidon2 over substrate-bn equals the node's, on these vectors.
    let c = p2::consts_from_jf(|f| bn::fr(&fr_le(f)));
    let (zero, one) = (substrate_bn::Fr::zero(), substrate_bn::Fr::one());
    for op in &ops {
        let pre: Vec<_> = op.preimage.iter().map(bn::fr).collect();
        assert!(p2::digest(&c, zero, one, &pre) == bn::fr(&op.note_id), "bn digest = node digest");
        let s: Vec<_> = op.siblings.iter().map(bn::fr).collect();
        let (from, to) = if op.insert { (zero, bn::fr(&op.note_id)) } else { (bn::fr(&op.note_id), zero) };
        let cmp = |a, b| p2::compress(&c, zero, a, b);
        assert!(root_from(from, op.position, &s, &cmp) == bn::fr(&op.old_root));
        assert!(root_from(to, op.position, &s, &cmp) == bn::fr(&op.new_root));
    }

    let one_case = vec![cases[0].clone()];
    write_job(out, "pol-ark", &Job::PolArk { vk: vk_bytes.clone(), pvk: pvk_bytes.clone(), proofs: cases.clone(), breakdown: true });
    write_job(out, "pol-ark-1", &Job::PolArk { vk: vk_bytes, pvk: pvk_bytes, proofs: one_case.clone(), breakdown: false });
    write_job(out, "pol-bn", &Job::PolBn { vk: bn_vk.clone(), proofs: cases.clone(), breakdown: true });
    write_job(out, "pol-bn-1", &Job::PolBn { vk: bn_vk, proofs: one_case, breakdown: false });
    write_job(out, "notes-ark", &Job::Notes { imp: 0, ops: ops.clone(), permutations: 64 });
    write_job(out, "notes-bn", &Job::Notes { imp: 1, ops: ops.clone(), permutations: 64 });
    write_job(out, "notes-bigint2", &Job::Notes { imp: 2, ops: ops.clone(), permutations: 64 });
    let span = slots.last().unwrap() - slots.first().unwrap();
    std::fs::write(
        out.join("vectors.json"),
        serde_json::to_string_pretty(&json!({
            "node": "logos-blockchain 3926a532b9d96d62747da064161a115a7c0c5a3e; circuits v0.5.7 (lbc-pol-sys prebuilt verification_key.json)",
            "vk_sha256_hex": hex::encode(<risc0_zkvm::sha::Impl as risc0_zkvm::sha::Sha256>::hash_bytes(lbc_pol_sys::artifacts::VERIFICATION_KEY).as_bytes()),
            "cases": doc,
            "testnet_headers": {"count": slots.len(), "first_slot": slots.first(), "last_slot": slots.last(), "slots_per_block": span as f64 / (slots.len() - 1) as f64},
            "note_ops": ops.iter().map(|o| json!({"insert": o.insert, "position": o.position, "old_root": hex::encode(o.old_root), "new_root": hex::encode(o.new_root)})).collect::<Vec<_>>(),
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!("ok: {} PoL cases, {} note ops, all checked against the node's code", cases.len(), ops.len());
}

fn run(guest: &Path, job: &Path, prove: bool) {
    use risc0_zkvm::{default_executor, default_prover, ExecutorEnv, ProverOpts};
    let elf = std::fs::read(guest).unwrap();
    let job = std::fs::read(job).unwrap();
    let mut input = (job.len() as u32).to_le_bytes().to_vec();
    input.extend_from_slice(&job);
    let env = ExecutorEnv::builder().write_slice(&input).build().unwrap();
    let t = Instant::now();
    let (journal, stats, extra) = if prove {
        let info = default_prover().prove_with_opts(env, &elf, &ProverOpts::succinct()).expect("a receipt");
        let secs = t.elapsed().as_secs_f64();
        let id = risc0_zkvm::compute_image_id(&elf).unwrap();
        info.receipt.verify(id).expect("the receipt verifies");
        (info.receipt.journal.bytes.clone(), (info.stats.segments, info.stats.total_cycles, info.stats.user_cycles), json!({"prove_seconds": secs, "receipt": "succinct", "image_id": id.to_string()}))
    } else {
        let s = default_executor().execute(env, &elf).expect("the guest runs");
        let segs = s.segments.len();
        let total: u64 = s.segments.iter().map(|g| 1u64 << g.po2).sum();
        (s.journal.bytes.clone(), (segs, total, s.cycles()), json!({"execute_seconds": t.elapsed().as_secs_f64()}))
    };
    let r: Report = borsh::from_slice(&journal).unwrap();
    println!(
        "{}",
        json!({
            "segments": stats.0, "total_cycles": stats.1, "user_cycles": stats.2,
            "steps": r.steps.iter().map(|(l, c)| json!([l, c])).collect::<Vec<_>>(),
            "verdicts": r.verdicts.iter().map(|(l, v)| json!([l, v])).collect::<Vec<_>>(),
            "run": extra,
        })
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("gen") => gen(Path::new(&a[2]), Path::new(&a[3])),
        Some("run") => run(Path::new(&a[2]), Path::new(&a[3]), a.get(4).map(String::as_str) == Some("--prove")),
        _ => eprintln!("pol-measure gen VECTOR.json OUT_DIR | pol-measure run GUEST.bin JOB [--prove]"),
    }
}
