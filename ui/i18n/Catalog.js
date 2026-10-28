.pragma library
.import "German.js" as German

function language(preferences) {
    for (const name of preferences || []) {
        const base = String(name).toLowerCase().split(/[-_.@]/)[0];
        if (base === "de" || base === "en") return base;
    }
    return "en";
}
function substitute(text, args) {
    return text.replace(/%([1-9][0-9]*)/g, (match, index) => Number(index) <= args.length ? String(args[Number(index) - 1]) : match);
}
function translate(source, locale, args) {
    const text = locale === "de" && Object.prototype.hasOwnProperty.call(German.messages, source) ? German.messages[source] : source;
    return substitute(text, args || []);
}
function message(value, locale, depth) {
    if (value === null || value === undefined) return "";
    const text = String(value);
    if (locale !== "de" || text.length > 65536) return text;
    if (Object.prototype.hasOwnProperty.call(German.messages, text)) return German.messages[text];
    if ((depth || 0) >= 4) return text;
    for (const pattern of compiled()) {
        const captures = pattern.regex.exec(text);
        if (!captures) continue;
        if (pattern.source.startsWith("{} matches")) {
            captures[4] += captures[5];
            captures[5] = "";
        }
        let index = 0;
        return German.messages[pattern.source].replace(/\{([^{}]*)\}/g, (_, slot) => {
            const position = slot ? pattern.slots.indexOf(slot) : index++;
            const captured = captures[position + 1];
            const nested = slot === "error" || pattern.source.includes("Recovery stopped") || pattern.source.includes("recovery stopped") || pattern.source.startsWith("{}. Source:") && position === 0 || pattern.source.startsWith("{} matches") && position >= 3;
            return nested ? message(captured, locale, (depth || 0) + 1) : captured;
        });
    }
    return text;
}

var patterns;
function escaped(value) {
    const slash = String.fromCharCode(92);
    let result = "";
    for (const ch of value) result += (".*+?^${}()|[]" + slash).includes(ch) ? slash + ch : ch;
    return result;
}
function compiled() {
    if (patterns) return patterns;
    patterns = [];
    for (const source of Object.keys(German.messages)) {
        const fields = /\{([^{}]*)\}/g;
        let match, previous = 0, expression = "^", slots = [];
        while ((match = fields.exec(source)) !== null) {
            expression += escaped(source.slice(previous, match.index)) + "([" + String.fromCharCode(92) + "s" + String.fromCharCode(92) + "S]*?)";
            slots.push(match[1]);
            previous = match.index + match[0].length;
        }
        if (!slots.length) continue;
        expression += escaped(source.slice(previous)) + "$";
        patterns.push({source: source, regex: new RegExp(expression), slots: slots});
    }
    // More-specific sentences take precedence over suffix-style error messages.
    patterns.sort((a, b) => b.source.replace(/\{[^{}]*\}/g, "").length - a.source.replace(/\{[^{}]*\}/g, "").length);
    return patterns;
}
