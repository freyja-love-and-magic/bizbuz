#!/usr/bin/env node

/**
 * BizBuz - Digital Business Cards
 *
 * Pulls from Planet Nine Prof profiles to create beautiful,
 * scannable digital business cards with vCard export.
 *
 * Flow:
 * 1. User shares their BizBuz link (by pubKey or emojicode)
 * 2. Recipient opens link, sees digital business card
 * 3. Recipient can scan QR or download vCard to save contact
 */

import express from 'express';
import QRCode from 'qrcode';
import sessionless from 'sessionless-node';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const app = express();
const PORT = process.env.PORT || 3013;

// Configuration
const PROF_BASE_URL = process.env.PROF_BASE_URL || 'http://localhost:3012';

console.log('');
console.log('  ____  _     ____            ');
console.log(' | __ )(_)___| __ ) _   _ ____');
console.log(' |  _ \\| |_  /  _ \\| | | |_  /');
console.log(' | |_) | |/ /| |_) | |_| |/ / ');
console.log(' |____/|_/___|____/ \\__,_/___|');
console.log('');
console.log(' Digital Business Cards');
console.log(' Powered by Planet Nine');
console.log('================================');
console.log(`Port: ${PORT}`);
console.log(`Prof URL: ${PROF_BASE_URL}`);
console.log('');

// Middleware
app.use(express.static(join(__dirname, 'public')));
app.use(express.json());

/**
 * GET / - Landing page
 */
app.get('/', async (req, res) => {
    const fs = await import('fs/promises');
    const landingPage = await fs.readFile(join(__dirname, 'public', 'index.html'), 'utf-8');
    res.send(landingPage);
});

/**
 * GET /card/:identifier - View business card
 *
 * identifier can be:
 * - pubKey (66 hex chars)
 * - emojicode (8 emojis)
 * - prof UUID
 */
app.get('/card/:identifier', async (req, res) => {
    try {
        const { identifier } = req.params;

        console.log(`Fetching business card for: ${identifier}`);

        // Fetch profile from Prof service
        const profile = await fetchProfile(identifier);

        if (!profile) {
            return res.status(404).send(generateErrorPage('Profile not found'));
        }

        // Generate the business card page
        const html = await generateBusinessCardPage(profile, identifier);
        res.send(html);

    } catch (error) {
        console.error('Error fetching card:', error);
        res.status(500).send(generateErrorPage(error.message));
    }
});

/**
 * GET /vcard/:identifier - Download vCard file
 */
app.get('/vcard/:identifier', async (req, res) => {
    try {
        const { identifier } = req.params;

        console.log(`Generating vCard for: ${identifier}`);

        // Fetch profile from Prof service
        const profile = await fetchProfile(identifier);

        if (!profile) {
            return res.status(404).json({ error: 'Profile not found' });
        }

        // Generate vCard
        const vcard = generateVCard(profile);

        // Set headers for download
        const filename = `${profile.name || 'contact'}.vcf`.replace(/[^a-zA-Z0-9.-]/g, '_');
        res.setHeader('Content-Type', 'text/vcard');
        res.setHeader('Content-Disposition', `attachment; filename="${filename}"`);
        res.send(vcard);

    } catch (error) {
        console.error('Error generating vCard:', error);
        res.status(500).json({ error: error.message });
    }
});

/**
 * GET /qr/:identifier - Generate QR code for card URL
 */
app.get('/qr/:identifier', async (req, res) => {
    try {
        const { identifier } = req.params;
        const cardUrl = `${req.protocol}://${req.get('host')}/card/${identifier}`;

        // Generate QR code as PNG
        const qrBuffer = await QRCode.toBuffer(cardUrl, {
            type: 'png',
            width: 300,
            margin: 2,
            color: {
                dark: '#10b981',  // Planet Nine green
                light: '#0a001a'  // Dark background
            }
        });

        res.setHeader('Content-Type', 'image/png');
        res.send(qrBuffer);

    } catch (error) {
        console.error('Error generating QR:', error);
        res.status(500).json({ error: error.message });
    }
});

/**
 * GET /api/profile/:identifier - Get profile as JSON
 */
app.get('/api/profile/:identifier', async (req, res) => {
    try {
        const { identifier } = req.params;
        const profile = await fetchProfile(identifier);

        if (!profile) {
            return res.status(404).json({ error: 'Profile not found' });
        }

        res.json({ success: true, profile });

    } catch (error) {
        console.error('Error fetching profile:', error);
        res.status(500).json({ error: error.message });
    }
});

/**
 * Fetch profile from Prof service
 */
async function fetchProfile(identifier) {
    try {
        // Try fetching by UUID first
        const response = await fetch(`${PROF_BASE_URL}/profile/${identifier}`);

        if (response.ok) {
            const data = await response.json();
            return data.profile || data;
        }

        // If not found, return demo profile for testing
        if (identifier === 'demo') {
            return getDemoProfile();
        }

        return null;

    } catch (error) {
        console.error('Prof fetch error:', error.message);

        // Return demo profile if Prof service unavailable
        if (identifier === 'demo') {
            return getDemoProfile();
        }

        return null;
    }
}

/**
 * Demo profile for testing
 */
function getDemoProfile() {
    return {
        name: 'Ada Lovelace',
        title: 'Software Enchantress',
        company: 'Planet Nine',
        email: 'ada@planetnine.app',
        phone: '+1 (555) 123-4567',
        website: 'https://planetnine.app',
        location: 'The Cosmos',
        bio: 'Building the future of privacy-first technology.',
        social: {
            github: 'freyja-love-and-magic',
            twitter: 'planetnine'
        }
    };
}

/**
 * Generate vCard 3.0 format
 */
function generateVCard(profile) {
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
        if (profile.social.github) {
            lines.push(`URL;TYPE=GitHub:https://github.com/${profile.social.github}`);
        }
        if (profile.social.twitter) {
            lines.push(`URL;TYPE=Twitter:https://twitter.com/${profile.social.twitter}`);
        }
        if (profile.social.linkedin) {
            lines.push(`URL;TYPE=LinkedIn:https://linkedin.com/in/${profile.social.linkedin}`);
        }
    }

    lines.push('END:VCARD');

    return lines.join('\r\n');
}

/**
 * Generate business card HTML page
 */
async function generateBusinessCardPage(profile, identifier) {
    // Generate QR code as data URL
    const cardUrl = `https://bizbuz.planetnine.app/card/${identifier}`;
    const qrDataUrl = await QRCode.toDataURL(cardUrl, {
        width: 150,
        margin: 1,
        color: {
            dark: '#10b981',
            light: '#1a0033'
        }
    });

    return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>${profile.name || 'BizBuz'} - Digital Business Card</title>
    <meta name="description" content="${profile.title || ''} at ${profile.company || 'Planet Nine'}">
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #0a001a 0%, #1a0033 50%, #0a001a 100%);
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;
            padding: 20px;
            color: white;
        }

        .card {
            background: rgba(255, 255, 255, 0.05);
            border: 1px solid rgba(16, 185, 129, 0.3);
            border-radius: 24px;
            padding: 40px;
            max-width: 400px;
            width: 100%;
            text-align: center;
            backdrop-filter: blur(10px);
            box-shadow:
                0 0 40px rgba(16, 185, 129, 0.1),
                0 0 80px rgba(139, 92, 246, 0.05);
            position: relative;
            overflow: hidden;
        }

        .card::before {
            content: '';
            position: absolute;
            top: -50%;
            left: -50%;
            width: 200%;
            height: 200%;
            background: radial-gradient(circle, rgba(16, 185, 129, 0.1) 0%, transparent 50%);
            animation: pulse 4s ease-in-out infinite;
        }

        @keyframes pulse {
            0%, 100% { opacity: 0.5; transform: scale(1); }
            50% { opacity: 1; transform: scale(1.1); }
        }

        .card-content {
            position: relative;
            z-index: 1;
        }

        .avatar {
            width: 100px;
            height: 100px;
            border-radius: 50%;
            background: linear-gradient(135deg, #10b981 0%, #059669 100%);
            margin: 0 auto 20px;
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 40px;
            font-weight: bold;
            color: white;
            box-shadow: 0 0 30px rgba(16, 185, 129, 0.4);
        }

        .name {
            font-size: 28px;
            font-weight: 700;
            margin-bottom: 8px;
            background: linear-gradient(135deg, #10b981 0%, #a78bfa 100%);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
            background-clip: text;
        }

        .title {
            font-size: 16px;
            color: rgba(167, 139, 250, 0.9);
            margin-bottom: 4px;
        }

        .company {
            font-size: 14px;
            color: rgba(255, 255, 255, 0.6);
            margin-bottom: 20px;
        }

        .bio {
            font-size: 14px;
            color: rgba(255, 255, 255, 0.7);
            line-height: 1.6;
            margin-bottom: 24px;
            font-style: italic;
        }

        .contact-info {
            text-align: left;
            margin-bottom: 24px;
        }

        .contact-item {
            display: flex;
            align-items: center;
            gap: 12px;
            padding: 10px 0;
            border-bottom: 1px solid rgba(255, 255, 255, 0.1);
            color: rgba(255, 255, 255, 0.8);
            text-decoration: none;
            transition: color 0.2s;
        }

        .contact-item:hover {
            color: #10b981;
        }

        .contact-item:last-child {
            border-bottom: none;
        }

        .contact-icon {
            width: 20px;
            text-align: center;
            color: #10b981;
        }

        .qr-section {
            margin-top: 24px;
            padding-top: 24px;
            border-top: 1px solid rgba(255, 255, 255, 0.1);
        }

        .qr-code {
            background: #1a0033;
            padding: 10px;
            border-radius: 12px;
            display: inline-block;
            margin-bottom: 12px;
        }

        .qr-code img {
            display: block;
        }

        .qr-label {
            font-size: 12px;
            color: rgba(255, 255, 255, 0.5);
        }

        .actions {
            display: flex;
            gap: 12px;
            margin-top: 24px;
        }

        .btn {
            flex: 1;
            padding: 14px 20px;
            border: none;
            border-radius: 12px;
            font-size: 14px;
            font-weight: 600;
            cursor: pointer;
            transition: all 0.2s;
            text-decoration: none;
            display: flex;
            align-items: center;
            justify-content: center;
            gap: 8px;
        }

        .btn-primary {
            background: linear-gradient(135deg, #10b981 0%, #059669 100%);
            color: white;
            box-shadow: 0 4px 20px rgba(16, 185, 129, 0.3);
        }

        .btn-primary:hover {
            transform: translateY(-2px);
            box-shadow: 0 6px 30px rgba(16, 185, 129, 0.4);
        }

        .btn-secondary {
            background: rgba(255, 255, 255, 0.1);
            color: white;
            border: 1px solid rgba(255, 255, 255, 0.2);
        }

        .btn-secondary:hover {
            background: rgba(255, 255, 255, 0.15);
        }

        .footer {
            margin-top: 30px;
            text-align: center;
            color: rgba(255, 255, 255, 0.4);
            font-size: 12px;
        }

        .footer a {
            color: #10b981;
            text-decoration: none;
        }

        /* Floating particles */
        .particle {
            position: fixed;
            width: 4px;
            height: 4px;
            background: #10b981;
            border-radius: 50%;
            opacity: 0.3;
            animation: float 10s infinite;
        }

        @keyframes float {
            0%, 100% { transform: translateY(100vh) rotate(0deg); opacity: 0; }
            10% { opacity: 0.3; }
            90% { opacity: 0.3; }
            100% { transform: translateY(-100vh) rotate(720deg); opacity: 0; }
        }
    </style>
</head>
<body>
    <!-- Floating particles -->
    <div class="particle" style="left: 10%; animation-delay: 0s;"></div>
    <div class="particle" style="left: 30%; animation-delay: 2s;"></div>
    <div class="particle" style="left: 50%; animation-delay: 4s;"></div>
    <div class="particle" style="left: 70%; animation-delay: 6s;"></div>
    <div class="particle" style="left: 90%; animation-delay: 8s;"></div>

    <div class="card">
        <div class="card-content">
            <div class="avatar">${getInitials(profile.name)}</div>

            <h1 class="name">${escapeHtml(profile.name || 'Anonymous')}</h1>
            ${profile.title ? `<p class="title">${escapeHtml(profile.title)}</p>` : ''}
            ${profile.company ? `<p class="company">${escapeHtml(profile.company)}</p>` : ''}
            ${profile.bio ? `<p class="bio">"${escapeHtml(profile.bio)}"</p>` : ''}

            <div class="contact-info">
                ${profile.email ? `
                <a href="mailto:${escapeHtml(profile.email)}" class="contact-item">
                    <span class="contact-icon">@</span>
                    <span>${escapeHtml(profile.email)}</span>
                </a>
                ` : ''}

                ${profile.phone ? `
                <a href="tel:${escapeHtml(profile.phone)}" class="contact-item">
                    <span class="contact-icon">#</span>
                    <span>${escapeHtml(profile.phone)}</span>
                </a>
                ` : ''}

                ${profile.website ? `
                <a href="${escapeHtml(profile.website)}" target="_blank" class="contact-item">
                    <span class="contact-icon">~</span>
                    <span>${escapeHtml(profile.website.replace(/^https?:\/\//, ''))}</span>
                </a>
                ` : ''}

                ${profile.location ? `
                <div class="contact-item">
                    <span class="contact-icon">*</span>
                    <span>${escapeHtml(profile.location)}</span>
                </div>
                ` : ''}
            </div>

            <div class="qr-section">
                <div class="qr-code">
                    <img src="${qrDataUrl}" alt="QR Code" width="150" height="150">
                </div>
                <p class="qr-label">Scan to save contact</p>
            </div>

            <div class="actions">
                <a href="/vcard/${identifier}" class="btn btn-primary" download>
                    Save Contact
                </a>
                <button class="btn btn-secondary" onclick="shareCard()">
                    Share
                </button>
            </div>
        </div>
    </div>

    <div class="footer">
        <p>Powered by <a href="https://planetnine.app">Planet Nine</a></p>
    </div>

    <script>
        async function shareCard() {
            const url = window.location.href;
            const title = '${escapeHtml(profile.name || 'Contact')} - Business Card';

            if (navigator.share) {
                try {
                    await navigator.share({ title, url });
                } catch (err) {
                    copyToClipboard(url);
                }
            } else {
                copyToClipboard(url);
            }
        }

        function copyToClipboard(text) {
            navigator.clipboard.writeText(text).then(() => {
                alert('Link copied to clipboard!');
            }).catch(() => {
                prompt('Copy this link:', text);
            });
        }
    </script>
</body>
</html>`;
}

/**
 * Generate error page
 */
function generateErrorPage(message) {
    return `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Error - BizBuz</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #0a001a 0%, #1a0033 100%);
            color: white;
            display: flex;
            align-items: center;
            justify-content: center;
            min-height: 100vh;
            margin: 0;
            padding: 20px;
        }
        .error {
            background: rgba(255, 255, 255, 0.05);
            padding: 40px;
            border-radius: 20px;
            text-align: center;
            border: 1px solid rgba(239, 68, 68, 0.3);
        }
        h1 { color: #ef4444; margin-bottom: 16px; }
        p { color: rgba(255, 255, 255, 0.7); }
        a { color: #10b981; }
    </style>
</head>
<body>
    <div class="error">
        <h1>Oops!</h1>
        <p>${escapeHtml(message)}</p>
        <p style="margin-top: 20px;"><a href="/">Back to BizBuz</a></p>
    </div>
</body>
</html>`;
}

/**
 * Get initials from name
 */
function getInitials(name) {
    if (!name) return '?';
    return name
        .split(' ')
        .map(word => word[0])
        .join('')
        .substring(0, 2)
        .toUpperCase();
}

/**
 * Escape HTML special characters
 */
function escapeHtml(str) {
    if (!str) return '';
    return String(str)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#039;');
}

// Start server
app.listen(PORT, () => {
    console.log(`BizBuz running on http://localhost:${PORT}`);
    console.log('');
    console.log('Endpoints:');
    console.log(`  GET /              - Landing page`);
    console.log(`  GET /card/:id      - View business card`);
    console.log(`  GET /vcard/:id     - Download vCard`);
    console.log(`  GET /qr/:id        - QR code image`);
    console.log(`  GET /api/profile/:id - Profile JSON`);
    console.log('');
    console.log('Try: http://localhost:' + PORT + '/card/demo');
    console.log('');
});
