from __future__ import annotations

import user_settings as _user_settings

PHYSICS_CATEGORIES: set[str] = {
"physics.acc-ph", "physics.ao-ph", "physics.app-ph", "physics.atm-clus", "physics.atom-ph", "physics.bio-ph", "physics.chem-ph", "physics.class-ph", "physics.comp-ph", "physics.data-an", "physics.ed-ph", "physics.flu-dyn", "physics.gen-ph", "physics.geo-ph", "physics.hist-ph", "physics.ins-det", "physics.med-ph", "physics.optics", "physics.plasm-ph", "physics.pop-ph", "physics.soc-ph", "physics.space-ph", "astro-ph", "cond-mat", "gr-qc", "hep-ex", "hep-lat", "hep-ph", "hep-th", "math-ph", "nlin", "nucl-ex", "nucl-th", "quant-ph"
}
MATH_CATEGORIES: set[str] = {
    "math.AC","math.AG","math.AP","math.AT","math.CA","math.CO","math.CT",
    "math.CV","math.DG","math.DS","math.FA","math.GM","math.GN","math.GR",
    "math.GT","math.HO","math.IT","math.KT","math.LO","math.MG","math.MP",
    "math.NA","math.NT","math.OA","math.OC","math.PR","math.QA","math.RA",
    "math.RT","math.SG","math.SP","math.ST"
}
CS_CATEGORES: set[str] = {
    "cs.AI", "cs.AR", "cs.CC", "cs.CE", "cs.CG", "cs.CL", "cs.CR", "cs.CV", 
    "cs.CY", "cs.DB", "cs.DC", "cs.DL", "cs.DM", "cs.DS", "cs.ET", "cs.FL",
    "cs.GL", "cs.GR", "cs.GT", "cs.HC", "cs.IR", "cs.IT", "cs.LG", "cs.LO",
    "cs.MA", "cs.MM", "cs.MS", "cs.NA", "cs.NE", "cs.NI", "cs.OH", "cs.OS", 
    "cs.PF", "cs.PL", "cs.RO", "cs.SC", "cs.SD", "cs.SE", "cs.SI", "cs.SY" 
}


KNOWN_CATEGORY_ALIASES: dict[str, str] = {
    # ── Live bidirectional aliases (both strings actively used on arXiv) ──────
    # Mathematical Physics: math-ph top-level archive ↔ math subject class
    "math.MP":  "math-ph",      "math-ph":  "math.MP",
    # Information Theory: math archive ↔ cs archive
    "math.IT":  "cs.IT",        "cs.IT":    "math.IT",
    # Numerical Analysis: math archive ↔ cs archive
    "math.NA":  "cs.NA",        "cs.NA":    "math.NA",
    # Statistics Theory: math archive ↔ stat archive
    "math.ST":  "stat.TH",      "stat.TH":  "math.ST",
    # Systems & Control: EESS archive (2017) ↔ legacy cs archive
    "eess.SY":  "cs.SY",        "cs.SY":    "eess.SY",
    # General Economics: econ archive (2017) ↔ legacy q-fin archive
    "econ.GN":  "q-fin.EC",     "q-fin.EC": "econ.GN",

    # ── Subsumed / legacy archive identifiers ────────────────────────────────
    # These old standalone archives are no longer active but their identifiers
    # still appear in paper metadata. Map them to the modern dotted form.
    "cmp-lg":   "cs.CL",        # Computation & Language → cs.CL
    "adap-org": "nlin.AO",      # Adaptation & Self-Organizing Systems → nlin.AO
    "comp-gas": "nlin.CG",      # Cellular Automata & Lattice Gases → nlin.CG
    "chao-dyn": "nlin.CD",      # Chaotic Dynamics → nlin.CD
    "solv-int": "nlin.SI",      # Exactly Solvable & Integrable Systems → nlin.SI
    "patt-sol": "nlin.PS",      # Pattern Formation & Solitons → nlin.PS
    "alg-geom": "math.AG",      # Algebraic Geometry → math.AG
    "dg-ga":    "math.DG",      # Differential Geometry → math.DG
    "funct-an": "math.FA",      # Functional Analysis → math.FA
    "q-alg":    "math.QA",      # Quantum Algebra & Topology → math.QA
    "mtrl-th":  "cond-mat.mtrl-sci",    # Materials Theory → cond-mat.mtrl-sci
    "supr-con": "cond-mat.supr-con",    # Superconductivity → cond-mat.supr-con
    "acc-phys": "physics.acc-ph",       # Accelerator Physics → physics.acc-ph
    "ao-sci":   "physics.ao-ph",        # Atmospheric-Oceanic Sciences → physics.ao-ph
    "atom-ph":  "physics.atom-ph",      # Atomic, Molecular & Optical → physics.atom-ph
    "bayes-an": "physics.data-an",      # Bayesian Analysis → physics.data-an
    "chem-ph":  "physics.chem-ph",      # Chemical Physics → physics.chem-ph
    "plasm-ph": "physics.plasm-ph",     # Plasma Physics → physics.plasm-ph
}


DEFAULT_CS_COLORS: dict[str, str] = {
    "cs.AI":"#5b9247","cs.AR":"#5b9250","cs.CC":"#5b9261","cs.CE":"#5b9263",
    "cs.CG":"#5b9265","cs.CL":"#5b926a","cs.CR":"#5b9270","cs.CV":"#5b9274",
    "cs.CY":"#5b9277","cs.DB":"#5b9270","cs.DC":"#5b9271","cs.DL":"#5b927a",
    "cs.DM":"#5b927b","cs.DS":"#5b9281","cs.ET":"#5b9292","cs.FL":"#5b929a",
    "cs.GL":"#5b92aa","cs.GR":"#5b92b0","cs.GT":"#5b92b2","cs.HC":"#5b92b1",
    "cs.IR":"#5b92d0","cs.IT":"#5b92d2","cs.LG":"#5b92f5","cs.LO":"#5b92fd",
    "cs.MA":"#5b92ff","cs.MM":"#5b930b","cs.MS":"#5b9311","cs.NA":"#5b930f",
    "cs.NE":"#5b9313","cs.NI":"#5b9317","cs.OH":"#5b9326","cs.OS":"#5b9331",
    "cs.PF":"#5b9334","cs.PL":"#5b933a","cs.RO":"#5b935d","cs.SC":"#5b9361",
    "cs.SD":"#5b9362","cs.SE":"#5b9363","cs.SI":"#5b9367","cs.SY":"#5b9377"
}


DEFULT_MATH_COLORS: dict[str, str] = {
    "math.AC": "#8abf5b", "math.AG": "#8abf5b", "math.AP": "#8abf5b", 
    "math.AT": "#8abf5b", "math.CA": "#8abf5b", "math.CO": "#8abf5b", 
    "math.CT": "#8abf5b", "math.CV": "#8abf5b", "math.DG": "#8abf5b", 
    "math.DS": "#8abf5b", "math.FA": "#8abf5b", "math.GM": "#8abf5b", 
    "math.GN": "#8abf5b", "math.GR": "#8abf5b", "math.GT": "#8abf5b", 
    "math.HO": "#8abf5b", "math.IT": "#8abf5b", "math.KT": "#8abf5b", 
    "math.LO": "#8abf5b", "math.MG": "#8abf5b", "math.MP": "#8abf5b", 
    "math.NA": "#8abf5b", "math.NT": "#8abf5b", "math.OA": "#8abf5b", 
    "math.OC": "#8abf5b", "math.PR": "#8abf5b", "math.QA": "#8abf5b", 
    "math.RA": "#81bf5a", "math.RT": "#8abf5b", "math.SG": "#8abf5b", 
    "math.SP": "#8abf5b", "math.ST": "#8abf5b"
}


DEFULT_PHYSICS_COLORS: dict[str, str] = {
    "nlin": "#920f8f", "gr-qc": "#f503ec", "ao-ph": "#f9f3b6",
    "ed-ph": "#f9f30a", "hep-th": "#182062", "hep-ph": "#17e062",
    "hep-ex": "#173061", "acc-ph": "#17df8e", "gen-ph": "#17e041",
    "geo-ph": "#17e051", "med-ph": "#17dfa1", "optics": "#124cac",
    "bio-ph": "#17e054", "app-ph": "#17e06b", "pop-ph": "#17e06c",
    "soc-ph": "#17df9b", "hep-lat": "#10a061", "math-ph": "#beb9d8",
    "nucl-th": "#feea19", "nucl-ex": "#10eea1", "flu-dyn": "#1820b8",
    "atom-ph": "#bf05fc", "comp-ph": "#bf33ae", "data-an": "#ce49cf",
    "hist-ph": "#bf7953", "ins-det": "#14209a", "chem-ph": "#befb3e",
    "astro-ph": "#d15aec", "quant-ph": "#d6081c", "cond-mat": "#8e74ae",
    "atm-clus": "#b103fc", "plasm-ph": "#cf578b", "space-ph": "#c657ce",
    "class-ph": "#d5577e"
}


# Research cluster colors
# Color intent:
#   HEP      → deep crimson/maroon (high energy tradition)
#   QIS      → blue-violet (quantum)
#   ML/DL    → teal-green (distinct from the math #8abf5b lime-green)
#   CV       → sky blue
#   NLP      → cyan-teal
#   CondMat  → slate purple (matches existing #8e74ae)
#   Astro    → deep indigo
#   MathPhys → warm amber
#   Robotics → orange-red
#   Crypto   → steel grey-blue
#   TCS      → olive/khaki
#   CompBio  → forest green
#   NucPhys  → brick red
#   NumSci   → dusty blue-grey
#   StatProb → goldenrod
#   AMO      → magenta-rose
# ---------------------------------------------------------------------------

RESEARCH_CLUSTER_COLORS: dict[str, str] = {
    "High Energy Physics":              "#8b0000",  # deep crimson / maroon
    "Quantum Information Science":      "#5b4fcf",  # blue-violet
    "Machine Learning":                 "#2a9d8f",  # teal-green
    "Computer Vision":                  "#4ea8de",  # sky blue
    "Natural Language Processing":      "#00b4d8",  # cyan-teal
    "Condensed Matter Physics":         "#7b5ea7",  # slate purple
    "Astrophysics & Cosmology":         "#1a1f6e",  # deep indigo
    "Mathematical Physics":             "#e9a825",  # warm amber
    "Robotics & Control":               "#e07334",  # orange-red
    "Cryptography & Security":          "#4a7c9e",  # steel grey-blue
    "Theoretical Computer Science":     "#8a8a3a",  # olive / khaki
    "Computational Biology":            "#2d6a4f",  # forest green
    "Nuclear Physics":                  "#a33b20",  # brick red
    "Numerical & Scientific Computing": "#6b7fa6",  # dusty blue-grey
    "Statistics & Probability":         "#c9a227",  # goldenrod
    "Atomic Molecular & Optical Physics": "#c2448f", # magenta-rose
}

RESEARCH_CLUSTER_CATEGORIES: dict[str, list[str]] = {
    # hep-ex / hep-lat / hep-ph / hep-th form the canonical HEP quad on arXiv
    "High Energy Physics": [
        "hep-ex", "hep-lat", "hep-ph", "hep-th",
    ],

    # quant-ph is the primary home; cs.ET covers quantum computing hardware/algorithms
    "Quantum Information Science": [
        "quant-ph", "cs.ET",
    ],

    # cs.LG (learning), cs.NE (neural/evolutionary), cs.AI (general AI)
    "Machine Learning": [
        "cs.LG", "cs.NE", "cs.AI",
    ],

    # cs.CV (vision), cs.GR (graphics, often co-submitted), cs.MM (multimedia)
    "Computer Vision": [
        "cs.CV", "cs.GR", "cs.MM",
    ],

    # cs.CL (computation & language), cs.IR (information retrieval / search)
    "Natural Language Processing": [
        "cs.CL", "cs.IR",
    ],

    # cond-mat is the top-level umbrella; nlin overlaps (complex systems)
    "Condensed Matter Physics": [
        "cond-mat", "nlin",
    ],

    # astro-ph covers all astrophysics sub-fields; gr-qc (general relativity / cosmology)
    "Astrophysics & Cosmology": [
        "astro-ph", "gr-qc",
    ],

    # math-ph and math.MP are canonical aliases for Mathematical Physics on arXiv;
    # both keys are kept because both appear independently in paper metadata
    "Mathematical Physics": [
        "math-ph", "math.MP",
    ],

    # cs.RO (robotics), cs.SY (systems & control), math.OC (optimization & control)
    "Robotics & Control": [
        "cs.RO", "cs.SY", "math.OC",
    ],

    # cs.CR is the dedicated security / cryptography category
    # cs.IT (information theory) overlaps here (coding theory, channel capacity)
    "Cryptography & Security": [
        "cs.CR", "cs.IT", "math.IT",
    ],

    # Complexity, data structures, logic, automata, combinatorics on graphs/words
    "Theoretical Computer Science": [
        "cs.CC", "cs.DS", "cs.LO", "cs.FL", "cs.DM", "cs.CG", "cs.GT",
    ],

    # NOTE: q-bio.* is not in the provided category universe.
    # Only physics.bio-ph (biophysics) and cs.CE (computational engineering,
    # which covers computational biology tools) are available.
    "Computational Biology": [
        "physics.bio-ph", "cs.CE",
    ],

    # nucl-ex (nuclear experiment) and nucl-th (nuclear theory) — sibling of HEP
    "Nuclear Physics": [
        "nucl-ex", "nucl-th",
    ],

    # Numerical methods and high-performance scientific computing
    "Numerical & Scientific Computing": [
        "math.NA", "cs.NA", "cs.SC", "physics.comp-ph",
    ],

    # Core probability and statistics; math.ST overlaps with cs data science
    "Statistics & Probability": [
        "math.ST", "math.PR",
    ],

    # Atoms, molecules, photons, lasers, and atomic clusters
    "Atomic Molecular & Optical Physics": [
        "physics.atom-ph", "physics.optics", "physics.atm-clus",
    ],
}


def _build_cat_color() -> dict[str, str]:
    overrides: dict[str, str] = _user_settings.get("cat_color_overrides") or {}
    return {
        **DEFULT_PHYSICS_COLORS,
        **DEFULT_MATH_COLORS,
        **DEFAULT_CS_COLORS,
        **overrides,
    }


# Single lookup table: physics → math → cs precedence, saved overrides win.
# Mutated in-place by set_color_override / remove_color_override so existing
# references stay valid after an override change.
CAT_COLOR: dict[str, str] = _build_cat_color()


def set_color_override(category: str, color: str) -> None:
    overrides: dict[str, str] = _user_settings.get("cat_color_overrides") or {}
    overrides[category] = color
    _user_settings.set("cat_color_overrides", overrides)
    CAT_COLOR.clear()
    CAT_COLOR.update(_build_cat_color())


def remove_color_override(category: str) -> None:
    overrides: dict[str, str] = _user_settings.get("cat_color_overrides") or {}
    overrides.pop(category, None)
    _user_settings.set("cat_color_overrides", overrides)
    CAT_COLOR.clear()
    CAT_COLOR.update(_build_cat_color())
