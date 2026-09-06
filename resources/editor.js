let hboxIsSaving = false;
let hboxIsDirty = false;

function cleanDocumentHtml(doc) {
    const clone = doc.documentElement.cloneNode(true);

    clone
        .querySelectorAll(".hboxUtils")
        .forEach((node) => node.remove());

    return "<!doctype html>\n" + clone.outerHTML.trim();
}

function cleanHtmlString(html) {
    const parsed = new DOMParser().parseFromString(html, "text/html");

    parsed
        .querySelectorAll(".hboxUtils")
        .forEach((node) => node.remove());

    return "<!doctype html>\n" + parsed.documentElement.outerHTML.trim();
}

function printHtmlDiff(a, b) {
    if (a === b) {
        console.log("No diff");
        return;
    }

    let index = 0;

    while (
        index < a.length &&
        index < b.length &&
        a[index] === b[index]
    ) {
        index++;
    }

    const context = 300;
    const start = Math.max(0, index - context);
    const endA = Math.min(a.length, index + context);
    const endB = Math.min(b.length, index + context);

    console.group("Hbox HTML diff");
    console.log("First difference at index:", index);
    console.log("Fresh length:", a.length);
    console.log("Current length:", b.length);

    console.log("Fresh around diff:");
    console.log(a.slice(start, endA));

    console.log("Current around diff:");
    console.log(b.slice(start, endB));

    console.groupEnd();
}

window.hboxSave = async function () {
    hboxIsSaving = true;

    try {
        const html = cleanDocumentHtml(document);

        const response = await fetch("/save", {
            method: "POST",
            headers: {
                "Content-Type": "text/html; charset=utf-8",
                "X-Hbox-Path": window.location.pathname,
            },
            body: html,
        });

        if (!response.ok) {
            console.error("Failed to save page", response.status);
            return;
        }

        console.log("Saved page");
    } finally {
        setTimeout(() => {
            hboxIsSaving = false;
        }, 1000);
    }
};

(() => {
    const intervalMs = 1000;
    const currentUrl = window.location.href;

    let lastServerHtml = null;

    async function fetchCleanHtml() {
        const response = await fetch(currentUrl, {
            method: "GET",
            cache: "no-store",
        });

        if (!response.ok) {
            return null;
        }

        return cleanHtmlString(await response.text());
    }

    async function checkForChanges() {
        if (hboxIsSaving) {
            return;
        }

        try {
            const freshHtml = await fetchCleanHtml();

            if (freshHtml === null) {
                return;
            }

            if (lastServerHtml === null) {
                lastServerHtml = freshHtml;
                return;
            }

            if (freshHtml !== lastServerHtml) {
                console.log("Hbox reload: server HTML changed");
                window.location.reload();
            }
        } catch (error) {
            console.warn("Live reload check failed:", error);
        }
    }

    setInterval(checkForChanges, intervalMs);
})();
