import type { MakeOptions, NewPasskey, PasskeySignIn, SignInOptions } from './api/client'

/**
 * Passkeys: what the browser's own API wants and gives, as this panel's API
 * sends and takes it. The browser deals in bytes, and the panel in the base64
 * that uses neither `+` nor `/`.
 */

function toBytes(text: string): Uint8Array<ArrayBuffer> {
  const plain = atob(text.replaceAll('-', '+').replaceAll('_', '/'))
  const bytes = new Uint8Array(plain.length)
  for (let at = 0; at < plain.length; at += 1) bytes[at] = plain.charCodeAt(at)
  return bytes
}

function toText(bytes: ArrayBuffer): string {
  let plain = ''
  for (const byte of new Uint8Array(bytes)) plain += String.fromCharCode(byte)
  return btoa(plain).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')
}

/**
 * Whether this page can have passkeys at all: a browser makes them only on a
 * page it trusts, which a panel reached by an address on the home network is not.
 */
export function passkeysHere(): boolean {
  return window.isSecureContext && 'PublicKeyCredential' in window
}

/** What went wrong at the device, in words. A person who closes the prompt has not made an error. */
function declined(error: unknown): Error {
  if (error instanceof DOMException && error.name === 'NotAllowedError') {
    return new Error('Nothing was confirmed on the device.')
  }
  if (error instanceof DOMException && error.name === 'InvalidStateError') {
    return new Error('This device has a passkey for this account already.')
  }
  return error instanceof Error ? error : new Error('The device gave no answer.')
}

/** Has the browser ask a device to make a passkey, and returns what the device answered. */
export async function makePasskey(options: MakeOptions): Promise<Omit<NewPasskey, 'name'>> {
  let made: Credential | null
  try {
    made = await navigator.credentials.create({
      publicKey: {
        challenge: toBytes(options.challenge),
        rp: { id: options.rp_id, name: 'Homewarp' },
        user: { id: toBytes(options.user_handle), name: options.username, displayName: options.username },
        // ES256, which every device can make, and the one kind Homewarp reads.
        pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
        // Kept on the device, so that signing in names nobody; and the device checks who holds it.
        authenticatorSelection: { residentKey: 'required', userVerification: 'required' },
        attestation: 'none',
        excludeCredentials: options.exclude.map((id) => ({ type: 'public-key', id: toBytes(id) })),
        timeout: 120_000,
      },
    })
  } catch (error) {
    throw declined(error)
  }
  if (!(made instanceof PublicKeyCredential)) throw new Error('The device made no passkey.')
  const response = made.response as AuthenticatorAttestationResponse
  return { client_data: toText(response.clientDataJSON), attestation: toText(response.attestationObject) }
}

/** Has the browser ask a device to sign in, and returns what the device signed. */
export async function signWithPasskey(options: SignInOptions): Promise<PasskeySignIn> {
  let got: Credential | null
  try {
    got = await navigator.credentials.get({
      publicKey: {
        challenge: toBytes(options.challenge),
        rpId: options.rp_id,
        userVerification: 'required',
        timeout: 120_000,
      },
    })
  } catch (error) {
    throw declined(error)
  }
  if (!(got instanceof PublicKeyCredential)) throw new Error('The device gave no passkey.')
  const response = got.response as AuthenticatorAssertionResponse
  return {
    id: toText(got.rawId),
    client_data: toText(response.clientDataJSON),
    authenticator_data: toText(response.authenticatorData),
    signature: toText(response.signature),
  }
}
