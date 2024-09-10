import { describe, test } from "vitest"

import { TLSSocket, connect } from "node:tls"

import { createMockClock } from "@dassie/lib-reactive"
import { createClock, createCrypto } from "@dassie/lib-reactive-io"

import { generateSelfSignedCertificate } from "../self-signed"
import { createMockDeterministicCryptoWithRsa } from "./mocks/deterministic-rsa"
import { DuplexPair } from "./mocks/duplex-pair"

describe("Self-signed certificate", () => {
  test("should generate a self-signed certificate", async ({ expect }) => {
    const certificate = await generateSelfSignedCertificate({
      clock: createMockClock(),
      crypto: createMockDeterministicCryptoWithRsa(),
      subject: {
        commonName: "example.com",
        organizationName: "Example Inc.",
        organizationalUnitName: "Example Inc. Unit",
        countryName: "XX",
        stateOrProvinceName: "XX",
        localityName: "Exampletown",
      },
      modulusLength: 256,
    })

    expect(certificate).toMatchInlineSnapshot(`
      {
        "certificate": "-----BEGIN CERTIFICATE-----
      MIICSTCCAfOgAwIBAgIUEZmc13Nz3iyXlKJbpVbzqvGoMhAwDQYJKoZIhvcNAQEL
      BQAweTELMAkGA1UEBhMCWFgxCzAJBgNVBAgMAlhYMRQwEgYDVQQHDAtFeGFtcGxl
      dG93bjEVMBMGA1UECgwMRXhhbXBsZSBJbmMuMRowGAYDVQQLDBFFeGFtcGxlIElu
      Yy4gVW5pdDEUMBIGA1UEAwwLZXhhbXBsZS5jb20wHhcNMDAwMTAxMDAwMDAwWhcN
      MDkxMjI5MDAwMDAwWjB5MQswCQYDVQQGEwJYWDELMAkGA1UECAwCWFgxFDASBgNV
      BAcMC0V4YW1wbGV0b3duMRUwEwYDVQQKDAxFeGFtcGxlIEluYy4xGjAYBgNVBAsM
      EUV4YW1wbGUgSW5jLiBVbml0MRQwEgYDVQQDDAtleGFtcGxlLmNvbTBcMA0GCSqG
      SIb3DQEBAQUAA0sAMEgCQQCYW74rudfUnBiV7HdtVCv58tLaWIcRzjVdDEoBGCkn
      Yf4EjipgM9/LSa5P9+bwqSkGTwkRZ/bxTSCofUSXXgNTAgMBAAGjUzBRMB0GA1Ud
      DgQWBBQf6inUSrXqs9DmmlruC26IznV0BTAfBgNVHSMEGDAWgBQf6inUSrXqs9Dm
      mlruC26IznV0BTAPBgNVHRMBAf8EBTADAQH/MA0GCSqGSIb3DQEBCwUAA0EAIar9
      c3EjLalTXheRoIMdq9/7DTy5ojvxMKI+DxxizUG+tOB9tN4h12ccYiKNhlziD3he
      9t2ovTnVrWkdoBhPDg==
      -----END CERTIFICATE-----",
        "privateKey": "-----BEGIN PRIVATE KEY-----
      MIIBVAIBADANBgkqhkiG9w0BAQEFAASCAT4wggE6AgEAAkEAmFu+K7nX1JwYlex3
      bVQr+fLS2liHEc41XQxKARgpJ2H+BI4qYDPfy0muT/fm8KkpBk8JEWf28U0gqH1E
      l14DUwIDAQABAkA5MQ8U6OEs7R+Jf8dg4dzUKaOe1OZ295ougINzH7erEVgrWD6g
      n8oUegwBYhNwGxpizQiqtSA6PNE1lEfVbghZAiEA0t04LjD4bnbREv778JF5YqM0
      HkBYGxXUsVa/OQHWS6UCIQC4+JEPioqNhGvICYZazzZ9/ja8/a91IpXL0JkJdDDB
      lwIgTKPzI3bRFPVCkHD1CT8Wq28+JjCk1VNd9wbcgc/VH7ECIQCjezWieb0ZdNdZ
      9mkj645q7vrsMH8Z1RZ2DAimXWYWQwIgZH/jZ10X9D0TLz4ilMZyV5x0KPakt7j2
      QifuBGDci3E=
      -----END PRIVATE KEY-----
      ",
      }
    `)
  })

  test("should be able to connect to a server using the generated certificate", async ({
    expect,
  }) => {
    const certificate = await generateSelfSignedCertificate({
      clock: createClock(),
      crypto: createCrypto(),
      subject: {
        commonName: "localhost",
        organizationName: "Example Inc.",
        organizationalUnitName: "Example Inc. Unit",
        countryName: "XX",
        stateOrProvinceName: "XX",
        localityName: "Exampletown",
      },
      modulusLength: 1024,
    })

    const duplexPair = new DuplexPair()

    // Server TLS setup
    const serverOptions = {
      key: certificate.privateKey,
      cert: certificate.certificate,
      isServer: true,
    }

    // Client TLS setup
    const clientOptions = {
      ca: certificate.certificate,
      socket: duplexPair.socket2,
    }

    // Server TLSSocket
    const serverTLS = new TLSSocket(duplexPair.socket1, serverOptions)
    serverTLS.setEncoding("utf8")

    // Client TLSSocket
    const clientTLS = connect(clientOptions)
    clientTLS.setEncoding("utf8")

    const result = await new Promise<string>((resolve, reject) => {
      serverTLS.on("error", reject)

      serverTLS.on("data", (data: string) => {
        serverTLS.write(`You said: ${data}`)
        serverTLS.end()
      })

      clientTLS.on("error", reject)

      clientTLS.on("secureConnect", () => {
        clientTLS.write("Hello from client!")
      })

      clientTLS.on("data", (data: string) => {
        clientTLS.end()
        resolve(data)
      })
    })

    expect(clientTLS.authorized).toBe(true)
    expect(result).toBe("You said: Hello from client!")
  })
})
