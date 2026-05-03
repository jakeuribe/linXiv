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


KNOWN_CATEGORY_ALIASES: dict[str,str] = {
    "math.MP": "math-ph", "math-ph": "math.MP"
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
    "ed-ph": "#f9f30a", "hep-th": "#1820622", "hep-ph": "#17e0622",
    "hep-ex": "#1730613", "acc-ph": "#17df8ec","gen-ph": "#17e0412",
    "geo-ph": "#17e0512", "med-ph": "#17dfa18","optics": "#124caca",
    "bio-ph": "#17e054d", "app-ph": "#17e06bc", "pop-ph": "#17e06bb",
    "soc-ph": "#17df9be","hep-lat": "#10a0613", "math-ph": "#beb9d8",
    "nucl-th": "#feea19", "nucl-ex": "#10eea19","flu-dyn": "#1820b81",
    "atom-ph": "#bf05fc", "comp-ph": "#bf33ae","data-an": "#ce49cf",
    "hist-ph": "#bf7953", "ins-det": "#14209a4", "chem-ph": "#befb3e",
    "astro-ph": "#d15aec","quant-ph": "#d6081c", "cond-mat": "#8e74ae",
    "atm-clus": "#b103fc", "plasm-ph": "#cf578b", "space-ph": "#c657ce",
    "class-ph": "#d5577e"
}


CAT_COLOR: dict[str, str] = {
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
