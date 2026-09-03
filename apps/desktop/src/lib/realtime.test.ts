import { describe, expect, it } from 'vite-plus/test';

import { describeRealtimeFailure, realtimeErrorDetail } from './realtime';

const endpoint = { scheme: 'http', host: '127.0.0.1', port: 8080 };

describe('realtime error detail', () => {
    it('reads the shape pusher-js delivers, with data as a JSON string', () => {
        const error = {
            type: 'WebSocketError',
            error: {
                type: 'PusherError',
                data: '{"code":4001,"message":"Application does not exist"}',
            },
        };

        expect(realtimeErrorDetail(error)).toEqual({
            code: 4001,
            message: 'Application does not exist',
        });
    });

    it('reads an already-parsed payload', () => {
        expect(realtimeErrorDetail({ data: { code: 4009, message: 'Handshake failed' } })).toEqual({
            code: 4009,
            message: 'Handshake failed',
        });
    });

    it('gives up quietly when the socket failed before any handshake', () => {
        expect(realtimeErrorDetail({ type: 'WebSocketError', error: {} })).toEqual({
            code: null,
            message: null,
        });
        expect(realtimeErrorDetail(undefined)).toEqual({ code: null, message: null });
    });

    it('takes a plain string as the message', () => {
        expect(realtimeErrorDetail('Connection reset')).toEqual({
            code: null,
            message: 'Connection reset',
        });
    });
});

describe('realtime failure text', () => {
    it('names the port clash behind an unknown application key', () => {
        const text = describeRealtimeFailure(
            { error: { data: { code: 4001, message: 'Application does not exist' } } },
            endpoint,
        );

        expect(text).toContain('127.0.0.1:8080');
        expect(text).toContain('does not recognise this application key');
    });

    it('keeps the code and message for anything else', () => {
        const text = describeRealtimeFailure({ data: { code: 4009, message: 'Nope' } }, endpoint);

        expect(text).toContain('code 4009');
        expect(text).toContain('Nope');
    });

    it('says the server could not be reached when there is nothing to report', () => {
        const text = describeRealtimeFailure({ type: 'WebSocketError', error: {} }, endpoint);

        expect(text).toContain('Could not reach the realtime server at 127.0.0.1:8080');
        expect(text).not.toContain('[object Object]');
    });
});
