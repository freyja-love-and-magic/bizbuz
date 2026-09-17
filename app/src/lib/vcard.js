/**
 * vCard 3.0 generation for BizBuz profiles.
 *
 * Pure, dependency-free functions so this module works identically in
 * server.js (Node) and the Tauri app's webview (browser), the two places
 * BizBuz turns a profile into a downloadable/shareable contact card.
 */

/**
 * Fold a vCard content line per RFC 2426 §2.6: no line may exceed 75
 * octets, and continuation lines start with a single leading space.
 * Without this, strict parsers (e.g. Apple Contacts) can reject or
 * truncate long lines such as an embedded base64 PHOTO.
 */
function foldLine(line) {
    const limit = 75;
    if (line.length <= limit) return line;

    let result = line.slice(0, limit);
    let rest = line.slice(limit);
    while (rest.length > 0) {
        result += '\r\n ' + rest.slice(0, limit - 1);
        rest = rest.slice(limit - 1);
    }
    return result;
}

/**
 * Generate vCard 3.0 format
 */
export function generateVCard(profile) {
    const lines = [
        'BEGIN:VCARD',
        'VERSION:3.0',
    ];

    if (profile.name) {
        lines.push(`FN:${profile.name}`);
        // Try to split into first/last name
        const parts = profile.name.split(' ');
        if (parts.length >= 2) {
            lines.push(`N:${parts.slice(1).join(' ')};${parts[0]};;;`);
        } else {
            lines.push(`N:;${profile.name};;;`);
        }
    }

    if (profile.title) {
        lines.push(`TITLE:${profile.title}`);
    }

    if (profile.company) {
        lines.push(`ORG:${profile.company}`);
    }

    if (profile.email) {
        lines.push(`EMAIL;TYPE=INTERNET:${profile.email}`);
    }

    if (profile.phone) {
        lines.push(`TEL;TYPE=CELL:${profile.phone}`);
    }

    if (profile.website) {
        lines.push(`URL:${profile.website}`);
    }

    if (profile.location) {
        lines.push(`ADR;TYPE=WORK:;;${profile.location};;;;`);
    }

    if (profile.bio) {
        lines.push(`NOTE:${profile.bio}`);
    }

    // Add social links as URLs
    if (profile.social) {
        if (profile.social.instagram) {
            lines.push(`URL;TYPE=Instagram:https://instagram.com/${profile.social.instagram}`);
        }
        if (profile.social.x) {
            lines.push(`URL;TYPE=X:https://x.com/${profile.social.x}`);
        }
        if (profile.social.tiktok) {
            lines.push(`URL;TYPE=TikTok:https://tiktok.com/@${profile.social.tiktok}`);
        }
        if (profile.social.youtube) {
            lines.push(`URL;TYPE=YouTube:https://youtube.com/@${profile.social.youtube}`);
        }
        if (profile.social.facebook) {
            lines.push(`URL;TYPE=Facebook:https://facebook.com/${profile.social.facebook}`);
        }
        if (profile.social.linkedin) {
            lines.push(`URL;TYPE=LinkedIn:https://linkedin.com/in/${profile.social.linkedin}`);
        }
        if (profile.social.github) {
            lines.push(`URL;TYPE=GitHub:https://github.com/${profile.social.github}`);
        }
        if (profile.social.codeberg) {
            lines.push(`URL;TYPE=Codeberg:https://codeberg.org/${profile.social.codeberg}`);
        }
    }

    if (profile.photo) {
        lines.push(foldLine(`PHOTO;ENCODING=b;TYPE=JPEG:${profile.photo}`));
    }

    lines.push('END:VCARD');

    return lines.join('\r\n');
}

/**
 * Sanitized .vcf filename for a profile, matching the convention already
 * used by the /vcard/:identifier download route.
 */
export function vcardFilename(profile) {
    return `${profile.name || 'contact'}.vcf`.replace(/[^a-zA-Z0-9.-]/g, '_');
}
