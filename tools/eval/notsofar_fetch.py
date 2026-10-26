"""Fetch 32 meetings of the NOTSOFAR-1 evaluation set (CC BY 4.0), one
single-channel far-field device each, and build their references: set
"notsofar" of docs/05 "Extend" (group pub-far).

NOTSOFAR-1 (Microsoft, CHiME-8 task 2) records real office meetings of 3 to
8 people (about 6 minutes each) in 30 rooms on commercial meeting devices.
This slice is `240629.1_eval_small_with_GT` of the Hugging Face dataset
microsoft/NOTSOFAR at a pinned revision (not gated): every meeting with 6
or 7 participants (15) plus every fourth of the others in meeting-id order
(17), 3.47 hours, 9 rooms. For each meeting one single-channel device
recording (`sc_<device>/ch0.wav`, 16 kHz mono) is taken, the devices taken
in turn so that all five device kinds appear (rockfall, meetup, plaza,
studio, panacast).

References: `gt_transcription.json`, the human transcription (cased and
punctuated text per utterance, speaker alias, and word times aligned on the
close-talk microphones). One row per timed word; markup such as <ST/>,
<FILL/> and <UNKNOWN/> is dropped. `words_pc` is the transcript's own token
(casing, punctuation) where it lines up with the timed word. The RTTM is
derived from the word rows (adjacent words of one speaker merged).

  uv run python -I notsofar_fetch.py               # about 0.4 GB
  uv run python -I notsofar_fetch.py --meetings MTG_32000

Output: .work/public/notsofar/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from common import read_rttm, read_seglst  # noqa: E402

SET = "notsofar"
REVISION = "ba8fd0f034ce185fe4d24f47e53b4b8194795f07"  # microsoft/NOTSOFAR, 21 Jun 2026
BASE = f"https://huggingface.co/datasets/microsoft/NOTSOFAR/resolve/{REVISION}/benchmark-datasets/eval_set/240629.1_eval_small_with_GT/MTG"
LICENCE = (
    "CC BY 4.0. Dataset card: \"The data is licensed under the Creative Commons Attribution 4.0 International License "
    "(CC BY 4.0).\" Repository not gated."
)
# (meeting, device, wav bytes, wav SHA-256 (the LFS oid), gt_transcription.json SHA-256,
#  gt_meeting_metadata.json SHA-256)
SELECTION = [
    ("MTG_32000", "sc_meetup_0", 11628288, "8b43d41c70b761cae7be761cfec5514d148548c37aa1b4f5ee9820f7f5d33b16", "fc75f3bf418f527051b904b6ee68ea97b003aba215da95160806392b3eda08bc", "88afece0ea9f4a5b32d28924eb6675e90ea5416e0abab821656e5bdd8ed33156"),
    ("MTG_32006", "sc_rockfall_2", 12255808, "b37c3d34462ca56006d67bd0aba434fc8e7df95534d971ac66ecb46a7b908805", "323cb14d9bb129d67645128bc5a65559c830b54982541bc577de86f199ce955b", "43cf2473716aa5dc958f16dfef82a5ea431a6ce5800cd695114de61b219187d0"),
    ("MTG_32020", "sc_plaza_0", 11475008, "6042188f303f31672a64f228a74b75d35c3612e65ba8d3d48c5f2fb8cdfe402e", "ce98ad1b16b728f3610807b6bffcbb52698a0c4c6561dfd70b21e7dd70ad5635", "e384bc0b0416767b63eb524e02fa578a9f99a03fc49b9a383262d1ed04d235fa"),
    ("MTG_32024", "sc_studio_0", 11488928, "b147c441c412c4a5eb1195c7b9b319415a1e750634b3d44bd05e4530aaa7738a", "0d6a9ec954f4ff888f25c83824e71523fd50ceaa9797b02607f2a98a07e0724a", "ec6afe8f387e5c3fac2de4d4926e3666959581dfd31c392305efe5fb447d7865"),
    ("MTG_32029", "sc_rockfall_1", 11675168, "acb083eb7b0a6e84531263f2820ca890d7c5e7bc616a430b7e1d5c11b5c32ac8", "800467e0fd0b1a3a273400d4510c0177fd4f6d43d5c14c49889dc9b6b26ce028", "52ee2d66833f1577e613a213fc402a0b07ddf07f12ab15af7ea335ee6a9667f1"),
    ("MTG_32042", "sc_studio_0", 11457248, "44df5da2fb06f06584fee3103239f5e284ce756de326f00851cda9affed3f5ce", "219260327b8ead0578f1d60a2815601a92536c17f9ebafd666ee27fc8f96a2f8", "e2ba573c607268b16c884e4059a14a52f5142e16e8e5b7517a3a76dc4c4d5f34"),
    ("MTG_32049", "sc_rockfall_1", 11420768, "b4c6009f2b833abb0f454416410314d3ab114fe8a6fff3a43f6ed9a2f1eeab81", "930849f617409d0ebd798670b271673c6c4d4655f2c48318604077be18b5903b", "773f61087738dc183d12bce14c0fc063ad717d29e145a4e9e02ad11bad9a9c68"),
    ("MTG_32063", "sc_rockfall_0", 11711008, "7593537807c0b23f4e1c59a01871408436afae0c243250ad6bcdb01f5f25feac", "37d9acacbeae27bf0026b7ad46d642283d755c419d781301acb004c1d6352e3a", "fda5369c2490447f0139b31eec22c4a1fed70a79187c0bcfb8fcde05028a31ec"),
    ("MTG_32071", "sc_meetup_0", 11654208, "59d90c2e048d5f3adefb020666cabea5c3df6ce2a453c295bc9cd69c28e909d7", "b57520aa3f6f7750fd378b9162a568296050bc4f66d4ace5ba85ae6b91ef50be", "6d8a0f4e84852b5d92c12a2ba01596535250f6edee9d6fa6cac286645d058c11"),
    ("MTG_32075", "sc_rockfall_2", 11483648, "eb65f1fd8c7d8eb30009c3ad4445268ca930784625357066d7d6092e09dedbc1", "ba3879d0e24c32e01835c07325a270fb223f0bc4c3aa2bc865e3de5440f451bd", "772c980615ff86f86cf2add23a930462dad89a3c0dd74e6d03a8ce00a23d3d3f"),
    ("MTG_32085", "sc_rockfall_2", 11477888, "d438e379f41aa5601faa1c801fefe32e0cea7d99bf579acc8c70afb9d2aeaed7", "92951d0a08a9bb6dfb090bb25110fa04d90f46a3d6e3289a0a13cce4bd0f7abd", "e6d9a1a72fb5504f5dd2a645839bf86ef3bd3bb95c963e69530489b104d8dcd0"),
    ("MTG_32090", "sc_studio_0", 11455968, "be73d04c226f5ef903a1bef27fd47080e724e9bf897a034462510bd8d9275a4c", "d133bcfdd2f78a14fe4ab7636789b79f2dcd47a8ad5242b612ecf930f46fcc27", "055afa11ad32b142134e19ad203a74154e03bdfb4718135fc9f3490f4dc971af"),
    ("MTG_32101", "sc_meetup_0", 15259808, "0eafea03fc85f35d2f32ac6460faa7e03110cf42ce28e6b2aed05eebcf726317", "dec3d6f8862abf6f3d9ec355d77aeb5a0cd7c254ea33bce0cc294f2477f1299f", "ff55cd45c00347e364bc42b755e3bfbaa390e60fd58b6d5a6a74871c56b42edc"),
    ("MTG_32102", "sc_rockfall_2", 15316608, "72028dbb14766456ce1a9b628dc1751ca4c2bd2bb94c2f726634a7ff727c8324", "7dc5b618f3c6964f811ea3b14d9ddb6bb942b43db6fb623333ba4f76f984b83a", "2baf084e722e7b2a348fa517b8bf66df2612c8cc41ccf7c91a8987d22475eb7f"),
    ("MTG_32105", "sc_meetup_0", 15324608, "8db03bbc71c97a84791494302648884ff963744ebb48c04c5ac75634fbb42bed", "f83ef8082bea41c3bfef6cda2e1432a5af368966f920379b1f6923f80e74a0dd", "1fda5e2ec67f5a0d32cd6ac3ed6a421c9c4b7dd144e754be6350e3f11e407674"),
    ("MTG_32106", "sc_plaza_0", 15306848, "5c77dfb26b477b061a39a807644aa6aa4d39408dbcadb59c077f539460ea203b", "288b40d3797c5dae41a15bd418ce18f8a58e8652f5263dea52f626944c0be777", "725f7f20f1a30e8e926d98b5b1ba9fffa57fcc475894b85a522ac6693a0b0a0d"),
    ("MTG_32107", "sc_meetup_0", 15310848, "e36da1470ef67624ffdb1e0695d6abec3c5a11d66d25ed4f6ff77b90166c3fff", "b2faeb7dd4b2379298ad9e2de094285ffa0393353a05f41f454d43f1b4dd232e", "f479ef785eaf615a3140fe48d05ceba23cdbf69eaacbfdad6cff414aef08f555"),
    ("MTG_32108", "sc_rockfall_1", 15286048, "c06ade576cf61b12bdc5f5801764523628a0094d95040c17b36bb740c8379f60", "39c64bf23487a65b4d72f2ef01bc9d9e0ceb74afdad86e38bcc5beca40924396", "cdfb8e9b9726c797f8381429fe20ad41a4b1fd89162df6b2b1923f81e6022294"),
    ("MTG_32109", "sc_plaza_0", 15340128, "78169b18d93c5e70f27a044a00621741df8b0bb7b93e5273dd160bfb17cef555", "059cbf4c0132a6f4a9633fb792745f591200dcb2069c61ce8ddddc67978bdaa6", "b2c70cce7925b7f067c4ea6fed574891837f3b121a766e7618638834565b9d0a"),
    ("MTG_32110", "sc_studio_0", 15318528, "8577cc89370f82d279f30918d07d47980261afa69a3c81b29794093357dc2720", "58a7bd5c381c9e7c751ab757a63a80ee10fe368defb845ba4fe20fc07fc888d5", "3aa58fd3989466654076658e572f683fd9720aa2f6077b82e0f32e313c21e051"),
    ("MTG_32178", "sc_meetup_0", 11461888, "b0b9ad9b6bc6c835e05222a2568867be5feb9dcf1ad713760e87a109d3abfa76", "adfe135007de6d429a968ad828a6acbb709b2efc7807030785ef6b0bfa2e8d59", "dbcb1379b178125bead12cad72bd3b1954ed666621a3e0e38586c70cf51e9a6c"),
    ("MTG_32179", "sc_rockfall_0", 11508448, "5883469c81100b3687a2dd9af4eff4da2b660fe61e0311b6b35557a9d5d63608", "588c429a7453aa6f11a8215f4ca44836ce27d3718fef434287e1a29700754e58", "184cb7c06636379d271872346ef01b752319d497024e2df3cfe0737c50685004"),
    ("MTG_32182", "sc_panacast_0", 11422848, "84acd017f5ff6fa023cedd8eb957deed915c1c049b4282fc1caae748957b6ebe", "648ac78d2b43a48810e57ca28753cf9a65f8fcf367dfc0afd49d3b07bab07f33", "b7fb52b37343a59f15ad9fd31b126bca6e7c2422bbf2eb0c91d287e5784e7179"),
    ("MTG_32183", "sc_plaza_0", 11708928, "4926a5df39dd259665fdd8701f6536c6302c31aff711e94aeed10105ac22b7fc", "7f0c99a0044b11566588923efcc96b8b67009ce39c283be26a2fff63efa53802", "17851348055250a1a2e5887ebd88f5c26b107e19b99158c5117f2cd8e90b0d28"),
    ("MTG_32185", "sc_panacast_0", 11511488, "3482560d6050fcc8613b40303ebbf52c3e57aae9abaa8fb26cc34de8b84b6d4c", "4a6ae2531c38333aeb8f8ff35c2c00959094118f1a770c867a33d3ad7a4e6e66", "ba6d5588841a3468cce34d2038007e47feecd4195d464cadab457cc64ab6a5b1"),
    ("MTG_32190", "sc_rockfall_0", 11498688, "31d5d25cd8b3b92b63f3ff0cca84d1ba1f3d5290960e93365ab5d12516087a0e", "975b082d61cdaa7a22d72c65ec858db243da663c9a476540fd13b3410470b421", "96e4d2304c40a3916b8484d401833f966a2bde5e63370598e509392cf316a08e"),
    ("MTG_32191", "sc_panacast_0", 11511488, "33b003c0604ee20331489e5ee0f8480f054f7eb3375e6d5196d041cf13754dab", "b9a023f9f9036e534f48a4e1d8f3f9af90f05f1f272101ad73efdebc1fd779fc", "e1391b375f8422f647e1241356cd785668dba3a40ff7312cbe0f9a99c113d97e"),
    ("MTG_32252", "sc_studio_0", 11463328, "541cee9638be0726de0ef40f0b03f65e6f162f67e605e5313504b0780eaa27f3", "a6af40717da2e9172d24b8e20ad1bdb64ee11141c35e18f796773990adb8584c", "996e0cbc604a7cbcb2afc686294e17de89c25d1e556761a2d5297bf0c1e2bcd5"),
    ("MTG_32260", "sc_rockfall_0", 11490048, "d6061d57f31a9b2fe93d3795cf5666054997de0929bcd7762f2d70e0a2e25c8c", "b1d02b6df282c2c879c6cb42c4cf6ea22a60059533c86872cd532315895f5ad7", "238e792dc47fe46e6eb80100f4739ada3ca32aec05fb261e30d57be7ecefc5d4"),
    ("MTG_32313", "sc_rockfall_2", 11496288, "a98427b677e7787095b2af176d4984465c33b4732c5fe2d25e11f573c3f58423", "cc64102221013181218ee7ba2e169cc8c6ab55067962dee360ece56fa4e681b5", "1f9e62275a14e549efc6358285e2296bfbb43ad9fa49ff22ac2a64ce9104892d"),
    ("MTG_32321", "sc_panacast_0", 11426208, "bfda766bfc673b9e39d2a9fdf122d62829ccb08346bb7b67b9ea4e656f5d5687", "a88b772a779a1a69416af6439dc4f87d0bac278fc2e0eeab0de1fb3f69164e80", "78ee4030ef6625cd243646f746931c75046412e5cf0e9ce2ecc64f840e03cfa1"),
    ("MTG_32346", "sc_plaza_0", 11478048, "af46ae02c51a321abba1680cace51ac3fe0899adec5cc2c6fc03c64a89c14028", "7f40fff71db8f528e5d367863ec4ab08fa9e2e19fe868055b76f99f1d2b0568b", "45750c0a266ff07327c80a7be6abc76a0cd9679273c453a0d306f0d9da8da957"),
]
_TAG = re.compile(r"</?[A-Za-z][^>]*>")


def _key(tok: str) -> str:
    return re.sub(r"[^\w']", "", tok.lower())


def meeting_rows(gt: list[dict]) -> list[dict]:
    rows = []
    for u in gt:
        timed = [w for w in u.get("word_timing") or [] if not _TAG.fullmatch(w[0].strip())]
        if not timed:
            continue
        toks = [t for t in _TAG.sub(" ", u.get("text") or "").split() if _key(t)]
        a = [_key(t) for t in toks]
        b = [_key(w[0]) for w in timed]
        pc: dict[int, str] = {}
        for blk in difflib.SequenceMatcher(a=a, b=b, autojunk=False).get_matching_blocks():
            for k in range(blk.size):
                pc[blk.b + k] = toks[blk.a + k]
        for i, (word, st, en) in enumerate(timed):
            tok = pc.get(i)
            row = {
                "speaker": u["speaker_id"],
                "start_time": round(float(st), 3),
                "end_time": round(max(float(en), float(st)), 3),
                "words": P.strip_punct(tok) if tok else word,
                "timing": "word",
            }
            if tok:
                row["words_pc"] = tok
            rows.append(row)
    return rows


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--meetings", help="comma-separated meeting ids (default: the 32 of the slice)")
    P.common_args(ap)
    a = ap.parse_args()
    want = {m.strip() for m in a.meetings.split(",")} if a.meetings else None
    root = P.set_dir(SET)
    sessions, inputs, stats = [], {}, {}
    for m, dev, size, wav_sha, gt_sha, md_sha in SELECTION:
        if want and m not in want:
            continue
        wav = P.fetch(f"{BASE}/{m}/{dev}/ch0.wav", root / "audio" / f"{m}.{dev}.wav", wav_sha, size=size,
                      allow_unpinned=a.allow_unpinned)
        gt_p = P.fetch(f"{BASE}/{m}/gt_transcription.json", root / "download" / f"{m}.gt_transcription.json", gt_sha,
                       size=300_000, allow_unpinned=a.allow_unpinned)
        md_p = P.fetch(f"{BASE}/{m}/gt_meeting_metadata.json", root / "download" / f"{m}.gt_meeting_metadata.json",
                       md_sha, size=1_000, allow_unpinned=a.allow_unpinned)
        inputs[f"{m}/{dev}/ch0.wav"] = wav_sha
        inputs[f"{m}/gt_transcription.json"] = gt_sha
        md = json.loads(md_p.read_text(encoding="utf-8"))
        sid = f"nsf-{m.lower().replace('_', '')}"
        seg, rt = root / "refs" / f"{sid}.ref.seglst.jsonl", root / "refs" / f"{sid}.ref.rttm"
        if a.rebuild or not (seg.is_file() and rt.is_file()):
            rows = meeting_rows(json.loads(gt_p.read_text(encoding="utf-8")))
            seg, rt = P.write_refs(root, sid, rows, P.rows_to_rttm(rows, sid, 0.0))
        stats[m] = {**P.ref_stats(read_seglst(seg), read_rttm(rt)), "participants": md.get("NumParticipants"),
                    "hashtags": md.get("Hashtags"), "room": md.get("Room"), "device": dev}
        sessions.append({
            "id": sid, "label": f"NOTSOFAR {m} {dev}", "group": "pub-far", "set": SET, "language": "en",
            "ref_source": "notsofar-word", "note": md.get("Hashtags"), "audio": wav, "ref_rttm": rt,
            "ref_seglst": seg, "duration_s": round(P.wav_duration(wav), 1),
        })
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": "NOTSOFAR-1 240629.1_eval_small_with_GT, 32 meetings, one single-channel device each",
        "licence": LICENCE,
        "sources": {"base": BASE},
        "sha256": inputs,
        "references": stats,
        "punctuation_casing": True,
    })
    if a.prune:
        P.prune([root / "download"])
    print(f"{len(sessions)} sessions, {sum(s['duration_s'] for s in sessions) / 3600:.2f} h in {root}")


if __name__ == "__main__":
    main()
