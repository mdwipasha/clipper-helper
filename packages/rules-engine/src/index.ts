import { CampaignTemplate, Composition } from '@clipper-helper/shared';

export interface RuleViolation {
  ruleId: string;
  type: string;
  severity: 'error' | 'warn';
  message: string;
}

export interface RuleCheckResult {
  passed: boolean;
  violations: RuleViolation[];
}

/**
 * Evaluates campaign rules against a composition.
 * Stub implementation for M1.0 scaffold; will be fully implemented in M2.
 */
export function evaluateRules(
  _template: CampaignTemplate,
  _composition: Composition
): RuleCheckResult {
  return {
    passed: true,
    violations: [],
  };
}
