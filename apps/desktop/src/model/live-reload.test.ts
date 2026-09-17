import { describe, expect, it } from 'vite-plus/test';

import { appleRunStopDescription, liveReloadUrlIssue } from './live-reload';

describe('iPhone live reload server', () => {
    it.each([
        'http://192.168.1.10:5173',
        '  http://10.0.0.2:8080/app/  ',
        'https://dev.example.test',
        'http://computer.local:5173',
        'http://[fd00::1]:5173',
    ])('accepts reachable server addresses such as %s', (url) => {
        expect(liveReloadUrlIssue(url, 'ios')).toBeNull();
    });

    it.each([
        'http://localhost:5173',
        'http://LOCALHOST.:5173',
        'http://app.localhost:5173',
        'http://127.0.0.1:5173',
        'http://127.12.34.56:5173',
        'http://127.1:5173',
        'http://2130706433:5173',
        'http://[::1]:5173',
        'http://[::ffff:127.0.0.1]:5173',
    ])('rejects a server that would point at the iPhone: %s', (url) => {
        expect(liveReloadUrlIssue(url, 'ios')).toContain('LAN address');
    });

    it.each([
        '',
        '192.168.1.10:5173',
        'file:///app',
        'ftp://dev.example.test',
        'http://user:secret@dev.example.test',
        'http://dev.example.test?token=secret',
        'http://dev.example.test#app',
        'http://dev.example.test?',
        'http://0.0.0.0:5173',
        'http://[::]:5173',
        'http://dev.example.test:0',
        'http://dev.example.test\n',
        'http://dev.example.test\\app',
    ])('rejects invalid or unsafe URLs on both platforms: %s', (url) => {
        expect(liveReloadUrlIssue(url, 'ios')).not.toBeNull();
        expect(liveReloadUrlIssue(url, 'android')).not.toBeNull();
    });

    it('keeps Android localhost support while requiring a network address on iPhone', () => {
        expect(liveReloadUrlIssue('http://localhost:5173', 'android')).toBeNull();
        expect(liveReloadUrlIssue('http://localhost:5173', 'ios')).not.toBeNull();
    });

    it('explains that Stop leaves the iPhone connected to its dev server', () => {
        expect(appleRunStopDescription('http://192.168.1.10:5173')).toContain(
            'live reload continues',
        );
        expect(appleRunStopDescription(null)).toContain('stays installed and running');
        expect(appleRunStopDescription(null)).not.toContain('dev server');
    });
});
