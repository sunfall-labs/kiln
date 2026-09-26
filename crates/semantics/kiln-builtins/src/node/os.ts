export const EOL: string = "\n";
export const devNull: string = "/dev/null";

export function arch(): string {
  if (typeof process !== "undefined" && process.arch) {
    return process.arch;
  }
  return "x64";
}

export function platform(): string {
  if (typeof process !== "undefined" && process.platform) {
    return process.platform;
  }
  return "linux";
}

export function type(): string {
  const p = platform();
  if (p === "darwin") return "Darwin";
  if (p === "win32") return "Windows_NT";
  return "Linux";
}

export function endianness(): string {
  return "LE";
}

export function homedir(): string {
  if (typeof process !== "undefined" && process.env) {
    if (process.env.HOME) return process.env.HOME;
    if (process.env.USERPROFILE) return process.env.USERPROFILE;
  }
  return "/";
}

export function tmpdir(): string {
  if (typeof process !== "undefined" && process.env) {
    if (process.env.TMPDIR) return process.env.TMPDIR;
    if (process.env.TEMP) return process.env.TEMP;
    if (process.env.TMP) return process.env.TMP;
  }
  return "/tmp";
}

export function hostname(): string {
  return "kiln";
}

export function machine(): string {
  return arch();
}

export function release(): string {
  return "0.0.0";
}

export function version(): string {
  return "kiln";
}

export function uptime(): number {
  return 0;
}

export function totalmem(): number {
  return 0;
}

export function freemem(): number {
  return 0;
}

export function loadavg(): number[] {
  return [0, 0, 0];
}

export function availableParallelism(): number {
  return 1;
}

export function cpus(): any[] {
  return [
    {
      model: "kiln",
      speed: 0,
      times: {
        user: 0,
        nice: 0,
        sys: 0,
        idle: 0,
        irq: 0
      }
    }
  ];
}

export function networkInterfaces(): any {
  return {
    loopback: [
      {
        address: "127.0.0.1",
        family: "IPv4",
        internal: true
      }
    ]
  };
}

export function userInfo(): any {
  return {
    uid: -1,
    gid: -1,
    username: "kiln",
    homedir: homedir(),
    shell: null
  };
}

const os: any = {
  EOL,
  arch,
  availableParallelism,
  cpus,
  default: null,
  devNull,
  endianness,
  freemem,
  homedir,
  hostname,
  loadavg,
  machine,
  networkInterfaces,
  platform,
  release,
  tmpdir,
  totalmem,
  type,
  uptime,
  userInfo,
  version
};
os.default = os;

export default os;
