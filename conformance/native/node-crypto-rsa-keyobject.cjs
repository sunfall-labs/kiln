const { createPrivateKey, createPublicKey, KeyObject } = require('node:crypto');

const privatePem = `-----BEGIN PRIVATE KEY-----
MIICeAIBADANBgkqhkiG9w0BAQEFAASCAmIwggJeAgEAAoGBANvbkjczD1Ti3NII
H2jKFXlBMgLx7Xf+A6IfxrJ8Jhk/bBjuBdhLi1/yKGvz/wxsZZRRUeu3G1llLjtY
paOsPVP/qixHh7okfdBlPab5pvf1QJCUeMtltPY8FxYU3RVvmlk3x7QdEdr8dFgz
9puXuMcBxlw8YCIJNXfw/YciypjhAgMBAAECgYEAhEC12OynGVoKmXF3a3K8rh8Q
tYLQZXDwwJK61N0nkeJ75RcxfuYaphGmTSIOV2vRQwIT9GZGVmUuBVcS9L0EeigL
EXRfiPb1qKC8bppjoc+M/zD9IUlQMep82AWIMNhpwf4YDh2uGGhnEM3pYvZG6T/3
ZBeHiqvtzvOk3GSSJ9ECQQD8FbEIHxKktWqAXRBx3Ot0cAlZkwGrNue+E0ewQYPF
+bfrepTLYFzJgETawSm83IwnS1A8XVT2CMdzwasZuWsdAkEA30W+EJBDdFY/mtRf
B7ndhTFKx9PiFwEujAybu8ID/HuYEk5ULD4yf8OUWhWMdIb/PgmWzYMVDd9jt9PS
/Kd1lQJASpA7+hP+1rtWkjt01skq2BkfmuAx2Sv3ZjCbvtXSHWgc78klY/bP20ZW
l4IVT2jJCAJt7D0LGW9FV8cMTiKbPQJBAMGYfZdrH7IuXvRnlQbj+NpWxVrDtFTy
J2Q0QozpS/+PQ2sMFBxQygTSlz4NVT5tF8C5G9OTB5yByLtKprWAlokCQQDVnp2j
Kj7KwmMv3MXJtMuO1P+ZSnPS5awkgAsOwi3vES9uhMyVXJpngQoTFUyNZtDIT50B
PN9JfEiZ4OiFjgDo
-----END PRIVATE KEY-----`;

const privateKey = createPrivateKey(privatePem);
const publicFromPrivate = createPublicKey(privateKey);
const privateFromDer = createPrivateKey({
  key: privateKey.export({ format: 'der', type: 'pkcs8' }),
  format: 'der',
  type: 'pkcs8',
});
const publicFromJwk = createPublicKey({
  key: {
    kty: 'RSA',
    n: '29uSNzMPVOLc0ggfaMoVeUEyAvHtd_4Doh_GsnwmGT9sGO4F2EuLX_Ioa_P_DGxllFFR67cbWWUuO1ilo6w9U_-qLEeHuiR90GU9pvmm9_VAkJR4y2W09jwXFhTdFW-aWTfHtB0R2vx0WDP2m5e4xwHGXDxgIgk1d_D9hyLKmOE',
    e: 'AQAB',
  },
  format: 'jwk',
});
const publicFromDer = createPublicKey({
  key: publicFromPrivate.export({ format: 'der', type: 'spki' }),
  format: 'der',
  type: 'spki',
});

console.log(privateKey instanceof KeyObject, privateKey.type, privateKey.asymmetricKeyType);
console.log(publicFromPrivate.type, publicFromPrivate.asymmetricKeyDetails.modulusLength);
console.log(privateFromDer.type, publicFromJwk.type, publicFromJwk.asymmetricKeyDetails.modulusLength);
console.log(publicFromDer.type, publicFromDer.asymmetricKeyDetails.modulusLength);
let missingExportFormatRejected = false;
try { privateKey.export(); } catch (error) { missingExportFormatRejected = error instanceof TypeError; }
let publicKeyObjectRejected = false;
try { createPublicKey(publicFromPrivate); } catch (error) { publicKeyObjectRejected = error instanceof TypeError; }
console.log(missingExportFormatRejected, publicKeyObjectRejected);
let pemMissingTypeRejected = false;
let derMissingTypeRejected = false;
try { privateKey.export({ format: 'pem' }); } catch (error) { pemMissingTypeRejected = error instanceof TypeError; }
try { privateKey.export({ format: 'der' }); } catch (error) { derMissingTypeRejected = error instanceof TypeError; }
const pkcs1Der = privateKey.export({ format: 'der', type: 'pkcs1' });
let derDeclaredTypeMismatchRejected = false;
try { createPrivateKey({ key: pkcs1Der, format: 'der', type: 'pkcs8' }); } catch { derDeclaredTypeMismatchRejected = true; }
let invalidMaterialIsGenericError = false;
try { createPrivateKey('test-secret'); } catch (error) {
  invalidMaterialIsGenericError = error instanceof Error && !(error instanceof TypeError);
}
console.log(pemMissingTypeRejected, derMissingTypeRejected,
  derDeclaredTypeMismatchRejected, invalidMaterialIsGenericError);
const largePublicKey = createPublicKey({
  key: { kty: 'RSA', n: Buffer.alloc(1024, 0xff).toString('base64url'), e: 'AQAB' },
  format: 'jwk',
});
const largeDerKey = createPublicKey({
  key: largePublicKey.export({ format: 'der', type: 'spki' }),
  format: 'der',
  type: 'spki',
});
const largePemKey = createPublicKey({
  key: largePublicKey.export({ format: 'pem', type: 'spki' }),
  format: 'pem',
  type: 'spki',
});
console.log(largeDerKey.asymmetricKeyDetails.modulusLength,
  largePemKey.asymmetricKeyDetails.modulusLength);
