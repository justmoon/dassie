import {
  concatUint8Arrays,
  hexToUint8Array,
  stringToUint8Array,
} from "uint8array-extras"

import { encodeField } from "./der"

export interface X509CertificateProperties {
  toBeSigned: Uint8Array
  signature: Uint8Array
}

export interface X509EntityProperties {
  countryName: string
  stateOrProvinceName: string
  localityName: string
  organizationName: string
  organizationalUnitName: string
  commonName: string
}

export interface X509CertificateSignableProperties {
  serialNumber: Uint8Array
  issuer: X509EntityProperties
  validFrom: Date
  validUntil: Date
  subject: X509EntityProperties
  subjectPublicKeyInfo: Uint8Array
  subjectKeyIdentifier: Uint8Array
  authorityKeyIdentifier: Uint8Array
}

const VERSION = hexToUint8Array("A003020102")
const SIGNATURE_ALGORITHM_SHA256_RSA = hexToUint8Array(
  "300D06092A864886F70D01010B0500",
)
const ATTRIBUTE_COUNTRY_NAME = hexToUint8Array("550406")
const ATTRIBUTE_STATE_OR_PROVINCE_NAME = hexToUint8Array("550408")
const ATTRIBUTE_LOCALITY_NAME = hexToUint8Array("550407")
const ATTRIBUTE_ORGANIZATION_NAME = hexToUint8Array("55040A")
const ATTRIBUTE_ORGANIZATIONAL_UNIT_NAME = hexToUint8Array("55040B")
const ATTRIBUTE_COMMON_NAME = hexToUint8Array("550403")
const EXTENSION_BASIC_CONSTRAINTS = hexToUint8Array(
  "300F0603551D130101FF040530030101FF",
)
const EXTENSION_SUBJECT_KEY_IDENTIFIER_OID = hexToUint8Array("0603551D0E")
const EXTENSION_AUTHORITY_KEY_IDENTIFIER_OID = hexToUint8Array("0603551D23")

export function serializeCertificate({
  toBeSigned,
  signature,
}: X509CertificateProperties) {
  return encodeField(
    0x30,
    concatUint8Arrays([
      toBeSigned,
      SIGNATURE_ALGORITHM_SHA256_RSA,
      encodeField(0x03, concatUint8Arrays([new Uint8Array(1), signature])),
    ]),
  )
}

export function serializeToBeSigned(
  signableProperties: X509CertificateSignableProperties,
) {
  return encodeField(
    0x30,
    concatUint8Arrays([
      // Version, see RFC 5280 section 4.1.2.1
      VERSION,

      // Serial Number, see RFC 5280 section 4.1.2.2
      encodeField(0x02, signableProperties.serialNumber),

      // Signature, see RFC 5280 section 4.1.2.3
      SIGNATURE_ALGORITHM_SHA256_RSA,

      // Issuer, see RFC 5280 section 4.1.2.4
      serializeEntity(signableProperties.issuer),

      // Validity, see RFC 5280 section 4.1.2.5
      encodeField(
        0x30,
        concatUint8Arrays([
          serializeValidityTime(signableProperties.validFrom),
          serializeValidityTime(signableProperties.validUntil),
        ]),
      ),

      // Subject, see RFC 5280 section 4.1.2.6
      serializeEntity(signableProperties.subject),

      // Subject Public Key Info, see RFC 5280 section 4.1.2.7
      signableProperties.subjectPublicKeyInfo,

      // Extensions, see RFC 5280 section 4.1.2.9
      encodeField(
        0xa3,
        encodeField(
          0x30,
          concatUint8Arrays([
            // Subject Key Identifier, see RFC 5280 section 4.2.1.2
            encodeField(
              0x30,
              concatUint8Arrays([
                EXTENSION_SUBJECT_KEY_IDENTIFIER_OID,
                encodeField(
                  0x04,
                  encodeField(0x04, signableProperties.subjectKeyIdentifier),
                ),
              ]),
            ),

            // Authority Key Identifier, see RFC 5280 section 4.2.1.1
            encodeField(
              0x30,
              concatUint8Arrays([
                EXTENSION_AUTHORITY_KEY_IDENTIFIER_OID,
                encodeField(
                  0x04,
                  encodeField(
                    0x30,
                    encodeField(
                      0x80,
                      signableProperties.authorityKeyIdentifier,
                    ),
                  ),
                ),
              ]),
            ),

            // Basic Constraints, see RFC 5280 section 4.2.1.9
            EXTENSION_BASIC_CONSTRAINTS,
          ]),
        ),
      ),
    ]),
  )
}

export function serializeEntity(entityProperties: X509EntityProperties) {
  return encodeField(
    0x30,
    concatUint8Arrays([
      serializeEntityField(
        ATTRIBUTE_COUNTRY_NAME,
        entityProperties.countryName,
        false,
      ),
      serializeEntityField(
        ATTRIBUTE_STATE_OR_PROVINCE_NAME,
        entityProperties.stateOrProvinceName,
      ),
      serializeEntityField(
        ATTRIBUTE_LOCALITY_NAME,
        entityProperties.localityName,
      ),
      serializeEntityField(
        ATTRIBUTE_ORGANIZATION_NAME,
        entityProperties.organizationName,
      ),
      serializeEntityField(
        ATTRIBUTE_ORGANIZATIONAL_UNIT_NAME,
        entityProperties.organizationalUnitName,
      ),
      serializeEntityField(ATTRIBUTE_COMMON_NAME, entityProperties.commonName),
    ]),
  )
}

export function serializeEntityField(
  type: Uint8Array,
  value: string,
  utf8 = true,
) {
  return encodeField(
    0x31,
    encodeField(
      0x30,
      concatUint8Arrays([
        encodeField(0x06, type),
        encodeField(utf8 ? 0x0c : 0x13, stringToUint8Array(value)),
      ]),
    ),
  )
}

export function serializeValidityTime(date: Date) {
  const fullYear = date.getUTCFullYear()
  if (fullYear > 9999) throw new Error("Year must not be above 9999")

  // Dates before 2050 are encoded as two-digit years
  // See RFC 5280 section 4.1.2.5.1
  const year =
    fullYear < 2050 ?
      String(date.getUTCFullYear() % 100).padStart(2, "0")
    : String(fullYear)
  const month = String(date.getUTCMonth() + 1).padStart(2, "0")
  const day = String(date.getUTCDate()).padStart(2, "0")
  const hour = String(date.getUTCHours()).padStart(2, "0")
  const minute = String(date.getUTCMinutes()).padStart(2, "0")
  const second = String(date.getUTCSeconds()).padStart(2, "0")

  return encodeField(
    0x17,
    stringToUint8Array(`${year}${month}${day}${hour}${minute}${second}Z`),
  )
}
