"""Fetch the SUMM-RE ASRU subset (French meetings, CC BY-SA 4.0) and build
its references: set "summre" of docs/05 "Extend" (group pub-lang).

SUMM-RE (LINAGORA and the Laboratoire Parole et Langage, Aix-Marseille) is
a corpus of meeting-style conversations in French: 3 or 4 participants,
about 20 minutes each, on a set topic (reporting, decision or planning
meetings), in a studio, at the lab or over Zoom, each participant on their
own microphone. The ASRU subset (datasets-CNRS/summ-re-asru on Hugging Face,
from Ortolang, not gated) is 10 meetings, 39 tracks, 3.6 hours, with
manually corrected transcriptions aligned to the word. Names in the audio
are anonymised.

Audio: the tracks of a meeting summed into one 16 kHz mono WAV (no level
normalisation), which is what a single device in the room would mix; the
in-room meetings therefore carry some crosstalk between microphones.
References: the TokensAlign tier of `gold_transcriptions/csv`: one row per
token with its aligned time; pauses (#), short pauses (+), laughter (@)
and noise (*) are dropped; joined forms (par_contre) are split. The
transcripts are lower case and unpunctuated, so there is no `words_pc`.
The RTTM is derived from the word rows (adjacent words merged, a pause
splits).

  uv run python -I summre_fetch.py --prune           # 4.4 GB download, 0.4 GB kept
  uv run python -I summre_fetch.py --meetings 018a_EARZ

--prune deletes each meeting's tracks once its mix is written.
Output: .work/public/summre/. Run with an isolated interpreter.
"""

from __future__ import annotations

import argparse
import csv
import io
import os
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import public_data as P  # noqa: E402
from common import read_rttm, read_seglst  # noqa: E402

SET = "summre"
REVISION = "de4e37a37f0052591049de6e5091144c0b9897be"  # datasets-CNRS/summ-re-asru, 29 Mar 2025
BASE = f"https://huggingface.co/datasets/datasets-CNRS/summ-re-asru/resolve/{REVISION}"
LICENCE = (
    "CC BY-SA 4.0. Dataset card: \"license: cc-by-sa-4.0\"; Ortolang citation: \"Licence Creative Commons - "
    "Attribution - partage dans les Mêmes Conditions 4.0 International\". Repository not gated."
)
WHERE = {"Z": "Zoom", "H": "studio", "L": "lab", "D": "home"}
# (track, wav bytes, wav SHA-256 (the LFS oid), gold csv SHA-256)
TRACKS = [
    ("018a_EARZ_055_anon", 116222594, "e59532958d7c0b29e59080ff077d2453834e450246047b86d10bfcfc054d4f31", "04627ba8cb110e5e73f2be08f42bef3b8f94e77725f3ff8a3a8f08912155371a"),
    ("018a_EARZ_056_anon", 116222594, "47f90f232844fe1305c626c339995a9ee686ee41aa2e7f53a7f2fd7af517194a", "7073d04b2ab97bbfdf045555d2777086aa883a59ecbcfed9d144e668cc16ea34"),
    ("018a_EARZ_057_anon", 116222594, "e74eebb0be968e28ee04f92943fe4457345c38164a741396e1be2cd6aab1c799", "5954ba96b3d53ee9f0763523fe69f455d1aaed6e729b3897322198d890841976"),
    ("018a_EARZ_058_anon", 116222594, "9511a7f92fae524e474b4370624496b1e3b61f65dcd375ec85c99600ebe89bbd", "45f0004791ad43c1cbe8aabfc50e275d1b1273c2a97d3be92d846e57ad70743d"),
    ("020b_EBDZ_017_anon", 97121202, "76918680d2f592b457d9f2bd6ff56b48467fb342f437e46e6ce89940a792e098", "9986939ebf20b0b015324bde2ace71f4c4740240449c05f044ceb336cd21936b"),
    ("020b_EBDZ_053_anon", 97121202, "968cbef618ec43b6797712fea5551f3c79c300f7d1e8a0f6fb4c28795be21624", "a0c633274c8928742a7b8bb0666e5cbc704cc3e79b002de4d1be8dcefb4b3137"),
    ("020b_EBDZ_057_anon", 97121202, "bd6568bdfd7c5f82637a39e10aef2691091be59ed83e3614818921721c2bf259", "95a604a866fa22986a54bcff829b60f071a7dcabcf57f156535a444a90033b30"),
    ("020b_EBDZ_063_anon", 97121202, "7e4dc091f4c0f93db1c729da50685193db9ee09d2bb9e151d93f574f7b448b4c", "69214a3dc946294c791f65a1f72545986895ae1104a4b824a8f7f441018940dd"),
    ("027a_EBRH_025_anon", 117047166, "44088c8c0962ad38e4c65ec1ab1c454ffce2f11150a2861feaa447e34915fdc0", "9824a1b4b1d9bca51ae353bf36c83e65b6225695a2ace2e82969c786d1641665"),
    ("027a_EBRH_075_anon", 117047166, "9eec4491bf75e8377f1e6e3b0c39c8f6ed7140fb8059d74021c9b76d9aeeb60c", "4c95557a09c128c82a3645668f401903a69bfaad6e656ba9a0b8650f9276aee1"),
    ("027a_EBRH_078_anon", 117047166, "4f670016177190b412853619a88156c27915544e094f6ad43572aafe1fdac1e8", "aeee0eec0114cab83593871728ed4581a14209ce389a6ab7e0dbc3949d7a933e"),
    ("032b_EADH_084_anon", 108147162, "de74caa13efc4b3bcd9c516f47e27191ff5542b4b5045d4eeabd04f744cf4bf3", "a5309fa06855c5fa2bcf171c28bd8eadfd30f2f9f6dd966ccfcbf239e92eb76c"),
    ("032b_EADH_085_anon", 108147162, "7b86e047cef1588a7e89dc548d9df79090e9dc994d20bdbbc44aec1470334578", "78fa31743de4e432e69c9cee9fb465c0b86dd24581e2c32f739c08c42dbdb0c3"),
    ("032b_EADH_086_anon", 108147162, "8973bb1c0b5fb8d36cebf4053e293a744dbc4d90c16db620d1048453bcc0539f", "9cc6ec1ca3bd2762632bc3cd816de6531ae4cafa928781116d6fce7ad07a3a84"),
    ("032b_EADH_087_anon", 108147162, "44f897938105d500de04f038d6ebb566871d77ce44d2741ec36ab2a2233008c1", "2e73a8a4ff08709415326c27c40c28889b5836a15d2f2efa2170508d2e39a663"),
    ("033a_EBRH_091_anon", 111787170, "0ed298f9c228a53669db3932684b3dc8987ba27dfa5c7bedda4f2369d3501347", "ac96c401e4abb44b43763f2559af2a57388eed1bc3ef840182d01217e65744d9"),
    ("033a_EBRH_092_anon", 121673200, "249c294dea18ba45ede5307e6b1bef025bd8c0f678cc8142bc66c338a69f3f16", "4b403e7ce26f3cc84978fb631a21d414a49b42f0ffce6226af8b8733dc66e282"),
    ("033a_EBRH_093_anon", 121673200, "35f6e63aaed09121407db087cf515459d7473f647fd6d294a40dceb5cce10125", "dd29a110108655fb6001403f12a2034d0f6deaff8982851426763bdad95f4b63"),
    ("033a_EBRH_094_anon", 121673200, "034bad003c882f39fc62e0bfa71e0422d08fd208902973e5db63420b30191387", "d2d14e8dd09794f202919f7725a1ae176e33c34cf9d7c05d9fa378e4abce4b24"),
    ("033c_EBPH_091_anon", 119257016, "d1112ad22aa70f5f448a70f9c96e380989d22dc852624c02201a1e37ff3312cb", "84e06ef1bfd22c2368581554ae2b34575c8a75f08e80858cc2944299d29e09ee"),
    ("033c_EBPH_092_anon", 109567302, "0b65c736d4fd4e9b3362682244cd0e9bb3755d4715e23f231f73dae11978ed5a", "b938816f0b70f8cc119581827d68ab39809aa960c76f40e658237bb7700bc22a"),
    ("033c_EBPH_093_anon", 109567302, "7d8ab42211d990561e0c8f61c6776ed8c4f50ca4a42284756e94b63d4bf8406e", "f6f5d454346c80942a144c958e281d9e8d9b895099431000e8898aeaac68afa7"),
    ("033c_EBPH_094_anon", 109567302, "05c38a10149b50bc3279d2ed5910d16ba0287fe0e2b86664fe411dc71459e967", "76e6b9dfc3897f47709ff6d0ca063816372a8de2cf36117face49afea4cf56bf"),
    ("034a_EBRH_095_anon", 110805750, "74d283a09f7530f4f2e5188ad63939ef851fc91cb4231342d0fe231ca841880b", "34904c2282b1054191d6b24e795d29ef35e83901a5e1ce8f43de3bd735dfa3a1"),
    ("034a_EBRH_096_anon", 110805750, "39516bd67ed7a19c12e0f27501e67c00302a5d241c9abff56ee918ce7134183e", "86f86321d9df891bd87bd95b0668a2bb6144b54170c8c688ccd30a42f84ba856"),
    ("034a_EBRH_097_anon", 110805750, "12359e7d61b2fcadbc54b57684efde19ae2c576e8dcf892444428ba2a20ac143", "aa1d5e7a5c079627793aecbb20af68e3e8c3ec465f4c0381c7664f62831cc7f6"),
    ("034a_EBRH_098_anon", 110805750, "6110eb73c9dacc1cb8459c8fa6ea0e793ff1a3f90d8e781ac752907105f907b6", "912750c81746bad5388444c86e12f16f3694afb3a7b52ad49cca53d9ec1e5599"),
    ("035b_EADH_088_anon", 104807434, "745e650860bb16f96986add5f9d19dd2b93adf579004e67bad7a6611b3138946", "ee696355f52f7f5e6f9af9faf5e61f3131e85d061814a8cf9e160efc536ec5dd"),
    ("035b_EADH_096_anon", 104807434, "a9db81078abcf89ac22de9b2af00a3daa33c2025aa0dee3be32ecbf50ee03fed", "1c955508e11398538dd54e5a7da6dacb73cdf48ebcc4e11d23efe176a84d28c0"),
    ("035b_EADH_097_anon", 104807434, "7ed64904d27debdf5189d055e4b61d96cf0e97fca469a01974d5ecff7ec65fc9", "910cb2f43df54da0dfb623a36b0ffe6d2153efa2375b1fcb3b605f73770aad41"),
    ("035b_EADH_098_anon", 104807434, "328e8b1309505d67fc4a09b140296e681f79302ff35419d6eb49e955aaad77d8", "8a866096806063bde78ca834a4746615688e8d1c832c977968a994acc7a366c6"),
    ("036c_EAPH_091_anon", 141803574, "1e42e121eaf1f4e6d11c3a6c0bb545d3b4fb3ba72275060c627327eceec90221", "78aaf5a8bfe9dbac6eec4fbe4db16442f0dd43b026a5f4088e2d5ab6908eec31"),
    ("036c_EAPH_092_anon", 141803574, "f0ecd0ea774252f184343270b3d90ce55773b4ac8450fa7438b3a7c7d6b13021", "60974ddbe41afd5be8f6abd8995d6ea39174a20aca68086b4541e8c46b61e64d"),
    ("036c_EAPH_093_anon", 141803574, "63ef2db02a538af5b0fadeae71c1fba46843c684dc05056efd612dfdcee8f117", "6972f7f9a11e01d70eb0e80f5a9be48e817a070643655ec049c2cc219eb25b84"),
    ("036c_EAPH_099_anon", 141803574, "b880ee630797656d4e59449f37be34445bccd632c92557888ebf14a26084f11d", "0e03bb814d5e37fb31267e4f48e2c69b107e346803b9a50e7dde623e94d6ce55"),
    ("069c_EEPL_156_anon", 111247426, "8acbb7f07001f7433bf527b28da50c2ac99170a55ff645606471ba2b0120194c", "a2b17b8e0ce7affab7616002a3288843317d1e24027635441bfb3911c7a94b97"),
    ("069c_EEPL_157_anon", 111247426, "bd2189bda238c4badb6b4063b7de95a20aa3f20bd1c816fd967c6e5d4bf6ee6b", "0f10a24908630a803b940c4365f3ddc5d4b774fcf6234c098474ad63feb42837"),
    ("069c_EEPL_158_anon", 111247426, "7a7f90b60e81f93d33e96e6fc3f02ec0fc8d17ff27d1779e8ee88fa7f69fa122", "53c119c6c7e0ec8a044202dc7ef05d5af65a0b6ebe616156d5db818e20b4e7d5"),
    ("069c_EEPL_159_anon", 111247426, "bfc77bacea1d65b818006bedc1000681b972b90588cc784ae7f37fdbfba4dcbd", "b2d38cf9763e77b8dd3c52e93d9165e3353f17c44dacdccd49e52cfef1d56286"),
]


def track_rows(text: str, speaker: str) -> list[dict]:
    rows = []
    for rec in csv.reader(io.StringIO(text.lstrip("\ufeff"))):
        if len(rec) < 4 or rec[0] != "TokensAlign":
            continue
        tok = rec[3].strip().replace("_", " ")
        if not any(ch.isalnum() for ch in tok):
            continue
        st, en = float(rec[1]), float(rec[2])
        rows.append({"speaker": speaker, "start_time": round(st, 3), "end_time": round(max(en, st), 3),
                     "words": tok, "timing": "word"})
    return rows


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--meetings", help="comma-separated meeting ids such as 018a_EARZ (default: all ten)")
    P.common_args(ap)
    a = ap.parse_args()
    meetings: dict[str, list[tuple]] = defaultdict(list)
    for t in TRACKS:
        meetings[t[0].rsplit("_", 2)[0]].append(t)
    want = {x.strip() for x in a.meetings.split(",")} if a.meetings else None
    root = P.set_dir(SET)
    sessions, inputs, stats = [], {}, {}
    for m, tracks in sorted(meetings.items()):
        if want and m not in want:
            continue
        wav = root / "audio" / f"{m}.wav"
        sid = f"summre-{m.lower()}"
        seg, rt = root / "refs" / f"{sid}.ref.seglst.jsonl", root / "refs" / f"{sid}.ref.rttm"
        raw = []
        if a.rebuild or not wav.is_file():
            for stem, size, wav_sha, _ in tracks:
                raw.append(P.fetch(f"{BASE}/audio_anonymized/{stem}.wav", root / "download" / f"{stem}.wav", wav_sha,
                                   size=size, allow_unpinned=a.allow_unpinned))
            P.to_wav16k(raw, wav)
        if a.rebuild or not (seg.is_file() and rt.is_file()):
            rows = []
            for stem, _, _, csv_sha in tracks:
                p = P.fetch(f"{BASE}/gold_transcriptions/csv/{stem}.csv", root / "download" / f"{stem}.csv", csv_sha,
                            size=300_000, allow_unpinned=a.allow_unpinned)
                rows += track_rows(p.read_text(encoding="utf-8"), stem.rsplit("_", 2)[1])
            seg, rt = P.write_refs(root, sid, rows, P.rows_to_rttm(rows, sid, 0.0))
        for stem, _, wav_sha, csv_sha in tracks:
            inputs[f"{stem}.wav"], inputs[f"{stem}.csv"] = wav_sha, csv_sha
        where = WHERE.get(m.split("_")[1][-1], "?")
        stats[m] = {**P.ref_stats(read_seglst(seg), read_rttm(rt)), "tracks": len(tracks), "recorded": where}
        sessions.append({
            "id": sid, "label": f"SUMM-RE {m} ({where})", "group": "pub-lang", "set": SET, "language": "fr",
            "ref_source": "summre-word", "audio": wav, "ref_rttm": rt, "ref_seglst": seg,
            "duration_s": round(P.wav_duration(wav), 1),
        })
        if a.prune:
            P.prune(raw)
    P.write_sessions(root, sessions)
    P.write_meta(root, {
        "set": SET,
        "content": "SUMM-RE ASRU subset: 10 French meetings, per-speaker tracks summed to one mix",
        "licence": LICENCE,
        "sources": {"base": BASE},
        "sha256": inputs,
        "references": stats,
        "punctuation_casing": False,
    })
    print(f"{len(sessions)} sessions, {sum(s['duration_s'] for s in sessions) / 3600:.2f} h in {root}")


if __name__ == "__main__":
    main()
