// Deterministic loopback-only node:dns implementation.

export const ADDRCONFIG = 32;
export const ADDRGETNETWORKPARAMS = 'EADDRGETNETWORKPARAMS';
export const ALL = 16;
export const BADFAMILY = 'EBADFAMILY';
export const BADFLAGS = 'EBADFLAGS';
export const BADHINTS = 'EBADHINTS';
export const BADNAME = 'EBADNAME';
export const BADQUERY = 'EBADQUERY';
export const BADRESP = 'EBADRESP';
export const BADSTR = 'EBADSTR';
export const CANCELLED = 'ECANCELLED';
export const CONNREFUSED = 'ECONNREFUSED';
export const DESTRUCTION = 'EDESTRUCTION';
export const EOF = 'EOF';
export const FILE = 'EFILE';
export const FORMERR = 'EFORMERR';
export const LOADIPHLPAPI = 'ELOADIPHLPAPI';
export const NODATA = 'ENODATA';
export const NOMEM = 'ENOMEM';
export const NONAME = 'ENONAME';
export const NOTFOUND = 'ENOTFOUND';
export const NOTIMP = 'ENOTIMP';
export const NOTINITIALIZED = 'ENOTINITIALIZED';
export const REFUSED = 'EREFUSED';
export const SERVFAIL = 'ESERVFAIL';
export const TIMEOUT = 'ETIMEOUT';
export const V4MAPPED = 8;

let currentOrder = 'verbatim';
let currentServers: string[] = ['127.0.0.1'];

export function getDefaultResultOrder(): string {
    return currentOrder;
}

export function setDefaultResultOrder(order: string): void {
    if (order !== 'ipv4first' && order !== 'verbatim') {
        throw new Error('invalid order');
    }
    currentOrder = order;
}

export function getServers(): string[] {
    return currentServers.slice();
}

export function setServers(servers: string[]): void {
    currentServers = Array.from(servers);
}

function makeDnsError(code: string, syscall: string, hostname?: string): Error {
    const err: any = new Error(`${syscall} ${code}${hostname ? ' ' + hostname : ''}`);
    err.code = code;
    err.syscall = syscall;
    if (hostname) {
        err.hostname = hostname;
    }
    return err;
}

export function lookup(
    hostname: string,
    optionsOrCallback?: any,
    callback?: any
): void {
    let opts: any = {};
    let cb: any;

    if (typeof optionsOrCallback === 'function') {
        cb = optionsOrCallback;
    } else {
        opts = optionsOrCallback || {};
        cb = callback;
    }

    if (typeof cb !== 'function') {
        throw new TypeError('The "callback" argument must be of type function');
    }

    const host = String(hostname || 'localhost');
    const all = Boolean(opts && opts.all);

    const addresses: Array<{ address: string; family: number }> = [];
    if (host === 'localhost' || host === '127.0.0.1') {
        addresses.push({ address: '127.0.0.1', family: 4 });
    }
    if (host === 'localhost' || host === '::1') {
        addresses.push({ address: '::1', family: 6 });
    }

    if (addresses.length === 0) {
        cb(makeDnsError('ENOTFOUND', 'getaddrinfo', host), all ? [] : undefined, all ? undefined : 0);
        return;
    }

    if (all) {
        cb(null, addresses);
    } else {
        cb(null, addresses[0].address, addresses[0].family);
    }
}

export function lookupService(
    address: string,
    port: number,
    callback: (err: Error | null, hostname: string, service: string) => void
): void {
    if (typeof callback !== 'function') {
        throw new TypeError('The "callback" argument must be of type function');
    }
    if (address === '127.0.0.1' || address === '::1' || address === 'localhost') {
        callback(null, 'localhost', String(port));
    } else {
        callback(makeDnsError('ENOTFOUND', 'getnameinfo', address), '', '');
    }
}

export function resolve(
    hostname: string,
    rrtypeOrCallback?: any,
    callback?: any
): void {
    let rrtype = 'A';
    let cb = callback;
    if (typeof rrtypeOrCallback === 'function') {
        cb = rrtypeOrCallback;
    } else if (typeof rrtypeOrCallback === 'string') {
        rrtype = rrtypeOrCallback;
    }

    if (typeof cb !== 'function') {
        throw new TypeError('The "callback" argument must be of type function');
    }

    if (rrtype === 'AAAA') {
        resolve6(hostname, cb);
    } else {
        resolve4(hostname, cb);
    }
}

export function resolve4(
    hostname: string,
    optionsOrCallback?: any,
    callback?: any
): void {
    const cb = typeof optionsOrCallback === 'function' ? optionsOrCallback : callback;
    if (typeof cb !== 'function') {
        throw new TypeError('The "callback" argument must be of type function');
    }
    if (hostname === 'localhost' || hostname === '127.0.0.1') {
        cb(null, ['127.0.0.1']);
    } else {
        cb(makeDnsError('ENOTFOUND', 'queryA', hostname), []);
    }
}

export function resolve6(
    hostname: string,
    optionsOrCallback?: any,
    callback?: any
): void {
    const cb = typeof optionsOrCallback === 'function' ? optionsOrCallback : callback;
    if (typeof cb !== 'function') {
        throw new TypeError('The "callback" argument must be of type function');
    }
    if (hostname === 'localhost' || hostname === '::1') {
        cb(null, ['::1']);
    } else {
        cb(makeDnsError('ENOTFOUND', 'queryAaaa', hostname), []);
    }
}

export function resolveCname(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryCname', hostname), []);
}

export function resolveMx(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryMx', hostname), []);
}

export function resolveNs(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryNs', hostname), []);
}

export function resolveTxt(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryTxt', hostname), []);
}

export function resolveSrv(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'querySrv', hostname), []);
}

export function resolvePtr(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryPtr', hostname), []);
}

export function resolveNaptr(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryNaptr', hostname), []);
}

export function resolveSoa(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'querySoa', hostname), {});
}

export function resolveCaa(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryCaa', hostname), []);
}

export function resolveTlsa(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(makeDnsError('ENODATA', 'queryTlsa', hostname), []);
}

export function resolveAny(hostname: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    callback(null, [{ type: 'A', address: '127.0.0.1', ttl: 300 }]);
}

export function reverse(ip: string, callback: any): void {
    if (typeof callback !== 'function') throw new TypeError('The "callback" argument must be of type function');
    if (ip === '127.0.0.1' || ip === '::1') {
        callback(null, ['localhost']);
    } else {
        callback(makeDnsError('ENOTFOUND', 'getHostByAddr', ip), []);
    }
}

export class Resolver {
    private _servers: string[] = ['127.0.0.1'];

    getServers(): string[] {
        return this._servers.slice();
    }

    setServers(servers: string[]): void {
        this._servers = Array.from(servers);
    }

    resolve(hostname: string, rrtypeOrCallback?: any, callback?: any): void {
        resolve(hostname, rrtypeOrCallback, callback);
    }

    resolve4(hostname: string, optionsOrCallback?: any, callback?: any): void {
        resolve4(hostname, optionsOrCallback, callback);
    }

    resolve6(hostname: string, optionsOrCallback?: any, callback?: any): void {
        resolve6(hostname, optionsOrCallback, callback);
    }

    resolveAny(hostname: string, callback: any): void {
        resolveAny(hostname, callback);
    }

    resolveCname(hostname: string, callback: any): void {
        resolveCname(hostname, callback);
    }

    resolveMx(hostname: string, callback: any): void {
        resolveMx(hostname, callback);
    }

    resolveNs(hostname: string, callback: any): void {
        resolveNs(hostname, callback);
    }

    resolveTxt(hostname: string, callback: any): void {
        resolveTxt(hostname, callback);
    }

    resolveSrv(hostname: string, callback: any): void {
        resolveSrv(hostname, callback);
    }

    resolvePtr(hostname: string, callback: any): void {
        resolvePtr(hostname, callback);
    }

    resolveNaptr(hostname: string, callback: any): void {
        resolveNaptr(hostname, callback);
    }

    resolveSoa(hostname: string, callback: any): void {
        resolveSoa(hostname, callback);
    }

    resolveCaa(hostname: string, callback: any): void {
        resolveCaa(hostname, callback);
    }

    resolveTlsa(hostname: string, callback: any): void {
        resolveTlsa(hostname, callback);
    }

    reverse(ip: string, callback: any): void {
        reverse(ip, callback);
    }

    cancel(): void {}
}

export const promises = {
    ADDRCONFIG,
    ADDRGETNETWORKPARAMS,
    ALL,
    BADFAMILY,
    BADFLAGS,
    BADHINTS,
    BADNAME,
    BADQUERY,
    BADRESP,
    BADSTR,
    CANCELLED,
    CONNREFUSED,
    DESTRUCTION,
    EOF,
    FILE,
    FORMERR,
    LOADIPHLPAPI,
    NODATA,
    NOMEM,
    NONAME,
    NOTFOUND,
    NOTIMP,
    NOTINITIALIZED,
    REFUSED,
    Resolver,
    SERVFAIL,
    TIMEOUT,
    V4MAPPED,
    getDefaultResultOrder,
    setDefaultResultOrder,
    getServers,
    setServers,
    lookup(hostname: string, options?: any): Promise<any> {
        return new Promise((res, rej) => {
            lookup(hostname, options, (err: any, addressOrAddresses: any, family?: number) => {
                if (err) {
                    rej(err);
                } else if (options && options.all) {
                    res(addressOrAddresses);
                } else {
                    res({ address: addressOrAddresses, family });
                }
            });
        });
    },
    lookupService(address: string, port: number): Promise<{ hostname: string; service: string }> {
        return new Promise((res, rej) => {
            lookupService(address, port, (err, hostname, service) => {
                if (err) rej(err);
                else res({ hostname, service });
            });
        });
    },
    resolve(hostname: string, rrtype?: string): Promise<any> {
        return new Promise((res, rej) => {
            resolve(hostname, rrtype, (err: any, records: any) => {
                if (err) rej(err);
                else res(records);
            });
        });
    },
    resolve4(hostname: string, options?: any): Promise<any> {
        return new Promise((res, rej) => {
            resolve4(hostname, options, (err: any, addresses: any) => {
                if (err) rej(err);
                else res(addresses);
            });
        });
    },
    resolve6(hostname: string, options?: any): Promise<any> {
        return new Promise((res, rej) => {
            resolve6(hostname, options, (err: any, addresses: any) => {
                if (err) rej(err);
                else res(addresses);
            });
        });
    },
    resolveAny(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveAny(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveCaa(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveCaa(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveCname(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveCname(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveMx(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveMx(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveNaptr(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveNaptr(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveNs(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveNs(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolvePtr(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolvePtr(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveSoa(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveSoa(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveSrv(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveSrv(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveTlsa(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveTlsa(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    resolveTxt(hostname: string): Promise<any> {
        return new Promise((res, rej) => {
            resolveTxt(hostname, (err: any, ret: any) => {
                if (err) rej(err);
                else res(ret);
            });
        });
    },
    reverse(ip: string): Promise<any> {
        return new Promise((res, rej) => {
            reverse(ip, (err: any, hostnames: any) => {
                if (err) rej(err);
                else res(hostnames);
            });
        });
    },
};

const dnsDefault = {
    ADDRCONFIG,
    ADDRGETNETWORKPARAMS,
    ALL,
    BADFAMILY,
    BADFLAGS,
    BADHINTS,
    BADNAME,
    BADQUERY,
    BADRESP,
    BADSTR,
    CANCELLED,
    CONNREFUSED,
    DESTRUCTION,
    EOF,
    FILE,
    FORMERR,
    LOADIPHLPAPI,
    NODATA,
    NOMEM,
    NONAME,
    NOTFOUND,
    NOTIMP,
    NOTINITIALIZED,
    REFUSED,
    Resolver,
    SERVFAIL,
    TIMEOUT,
    V4MAPPED,
    getDefaultResultOrder,
    setDefaultResultOrder,
    getServers,
    setServers,
    lookup,
    lookupService,
    promises,
    resolve,
    resolve4,
    resolve6,
    resolveAny,
    resolveCaa,
    resolveCname,
    resolveMx,
    resolveNaptr,
    resolveNs,
    resolvePtr,
    resolveSoa,
    resolveSrv,
    resolveTlsa,
    resolveTxt,
    reverse,
};

export default dnsDefault;
