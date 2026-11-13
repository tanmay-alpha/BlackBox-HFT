import { DASH, fmtSigned } from '../../components/fmt';

/** Quote currency is not named by the instrument contract; do not invent a currency symbol. */
export function fmtQuoteSigned(value: number | null): string {
  return value === null ? DASH : `${fmtSigned(value)} quote`;
}
