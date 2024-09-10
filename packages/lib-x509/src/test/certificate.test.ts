import { hexToUint8Array } from "uint8array-extras"
import { describe, test } from "vitest"

import { createCrypto } from "@dassie/lib-reactive-io"

import {
  type X509EntityProperties,
  serializeCertificate,
  serializeToBeSigned,
} from "../certificate"
import { serializePem } from "../pem"

const CERTIFICATE = `-----BEGIN CERTIFICATE-----
MIIC0jCCAjugAwIBAgIURBERXHHYxUwu2ERg6ZKSQYfeazwwDQYJKoZIhvcNAQEL
BQAwezELMAkGA1UEBhMCWFgxEjAQBgNVBAgMCVN0YXRlTmFtZTERMA8GA1UEBwwI
Q2l0eU5hbWUxFDASBgNVBAoMC0NvbXBhbnlOYW1lMRswGQYDVQQLDBJDb21wYW55
U2VjdGlvbk5hbWUxEjAQBgNVBAMMCWxvY2FsaG9zdDAeFw0yNDA5MTAxODU5NDla
Fw0zNDA5MDgxODU5NDlaMHsxCzAJBgNVBAYTAlhYMRIwEAYDVQQIDAlTdGF0ZU5h
bWUxETAPBgNVBAcMCENpdHlOYW1lMRQwEgYDVQQKDAtDb21wYW55TmFtZTEbMBkG
A1UECwwSQ29tcGFueVNlY3Rpb25OYW1lMRIwEAYDVQQDDAlsb2NhbGhvc3QwgZ8w
DQYJKoZIhvcNAQEBBQADgY0AMIGJAoGBAM9c9xSHP7SU4n82sXZPTnK2xrpefBlI
mas/Yl1Mv+YTkXJZKBGPMAbPiLGsesntoWHWzSReN5F3AVRUfJHar2YuPdjdtD6L
JDJoO2igC2+lQGwazU22rKj69XAFCXe2nhpL36ulNoMn3A1Xx3Oxih6El1wTO/gZ
nxFz8StpYcwdAgMBAAGjUzBRMB0GA1UdDgQWBBQX2j/yL7JzbsmsTYaljLnCidAg
8TAfBgNVHSMEGDAWgBQX2j/yL7JzbsmsTYaljLnCidAg8TAPBgNVHRMBAf8EBTAD
AQH/MA0GCSqGSIb3DQEBCwUAA4GBAArye5xbnLAvw6e3+rL9/UwhL28TP5Iue8Jz
Blmguu7rPbzflZpa06wz2HVvSU6uLg8du0IX8lCEbTR9NIzjFHoT6mipPJjhx+/w
eEvZQFraIGQIEwhQwKqvq1uLmz1WS+3Udi7CX9HpXiVEL9j5qRjD+XDf/WC5Uga6
7nH7A+Im
-----END CERTIFICATE-----`

const KEY = `-----BEGIN PRIVATE KEY-----
MIICdgIBADANBgkqhkiG9w0BAQEFAASCAmAwggJcAgEAAoGBAM9c9xSHP7SU4n82
sXZPTnK2xrpefBlImas/Yl1Mv+YTkXJZKBGPMAbPiLGsesntoWHWzSReN5F3AVRU
fJHar2YuPdjdtD6LJDJoO2igC2+lQGwazU22rKj69XAFCXe2nhpL36ulNoMn3A1X
x3Oxih6El1wTO/gZnxFz8StpYcwdAgMBAAECgYBLt0xY6JGwZHTXsqbV5ltks6yY
vItfyAykZP6LxsyDPD3tnPRuhPQqaHknNy2Wsfgte3tofiaYLPYFTtw1fb5xTqZO
V5DxixIReGVNaeY+1w09iWbORCFi8l9vrJ5MzoaIgr73Zl9eMaEF4ZljtCjx5l8U
+dfhISRK4mP/0qVQwQJBAPAXwcwSR437Ux+LaEEY6eNDcpDAYeLHfQaL50REMTdB
2v0GsG2SaWJ2a71fXWYY2+wuM0dIgGRhhPGehdl+hXkCQQDdGhOVLmp7ok8B7GzQ
ImttWIVVBzZL1+qM8DhIdnAwX9Sob+fBGbT8is0Ho77w0xSv+nb8+x3rp+2d9Ojj
NkbFAkEAlUpAcBNhuPBw1GmL7wZ90JnM8CMf+rKOlHaD3FgGvlRNxg3VwJxMuTPn
dH6LBgQpI+fqbMWxSDm3P8KRehcJCQJAabptgNn4S3S7CTVwdzruWhSJdbaELVJr
s8evcl9ImKlKvNz+WuWbGWSaVLvls62MZ/aCbcj7btQXmYAi2Xv77QJAF6urmsMF
qgFvEmk5+kcPSu2rJu8ws+zVwMaphfzC8d2nLQhdU4Gru0mHnZtL/JfKjyL9mSGf
ILpV/Vnwa5PvvQ==
-----END PRIVATE KEY-----`

describe("Certificate", () => {
  test("should be able to serialize a certificate", async ({ expect }) => {
    const crypto = createCrypto()

    const subject: X509EntityProperties = {
      commonName: "localhost",
      organizationName: "CompanyName",
      organizationalUnitName: "CompanySectionName",
      localityName: "CityName",
      countryName: "XX",
      stateOrProvinceName: "StateName",
    }

    const toBeSigned = serializeToBeSigned({
      subject,
      issuer: subject,
      validFrom: new Date("2024-09-10 18:59:49Z"),
      validUntil: new Date("2034-09-08 18:59:49Z"),
      subjectPublicKeyInfo: hexToUint8Array(
        "30819F300D06092A864886F70D010101050003818D0030818902818100CF5CF714873FB494E27F36B1764F4E72B6C6BA5E7C194899AB3F625D4CBFE61391725928118F3006CF88B1AC7AC9EDA161D6CD245E3791770154547C91DAAF662E3DD8DDB43E8B2432683B68A00B6FA5406C1ACD4DB6ACA8FAF570050977B69E1A4BDFABA5368327DC0D57C773B18A1E84975C133BF8199F1173F12B6961CC1D0203010001",
      ),
      serialNumber: hexToUint8Array("4411115C71D8C54C2ED84460E992924187DE6B3C"),
      authorityKeyIdentifier: hexToUint8Array(
        "17DA3FF22FB2736EC9AC4D86A58CB9C289D020F1",
      ),
      subjectKeyIdentifier: hexToUint8Array(
        "17DA3FF22FB2736EC9AC4D86A58CB9C289D020F1",
      ),
    })

    const keyPair = await crypto.importRsaKeyPair(KEY)

    const signature = await keyPair.sign(toBeSigned)

    const certificateData = serializeCertificate({
      toBeSigned,
      signature,
    })

    const certificate = serializePem(certificateData, "CERTIFICATE")

    expect(certificate).toBe(CERTIFICATE)
  })
})
