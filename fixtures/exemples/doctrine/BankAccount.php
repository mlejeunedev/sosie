<?php

declare(strict_types=1);

namespace App\Entity;

use Doctrine\ORM\Mapping as ORM;
use Symfony\Component\Validator\Constraints as Assert;

/**
 * Expected from the Doctrine scanner:
 *   - table `bank_account`
 *   - accountNumber → `iban` column (explicit name:) → preset iban (Assert\Iban)
 *   - swift         → `bic` column                      → preset bic  (Assert\Bic)
 *   - holderName    → `holder_name` column              → full_name (name)
 *   - isDefault     → keep (bool)
 *   - ManyToOne user relation → FK edge bank_account.user_id → user.id
 *
 * Trap: the property name `accountNumber` doesn't say "iban", the
 * validation attribute must win.
 */
#[ORM\Entity]
#[ORM\Table(name: 'bank_account')]
class BankAccount
{
    #[ORM\Id]
    #[ORM\GeneratedValue]
    #[ORM\Column]
    private ?int $id = null;

    #[ORM\ManyToOne(inversedBy: 'bankAccounts')]
    #[ORM\JoinColumn(nullable: false)]
    private ?User $user = null;

    #[ORM\Column(name: 'iban', length: 34)]
    #[Assert\Iban]
    private ?string $accountNumber = null;

    #[ORM\Column(name: 'bic', length: 11, nullable: true)]
    #[Assert\Bic]
    private ?string $swift = null;

    #[ORM\Column(length: 200)]
    private ?string $holderName = null;

    #[ORM\Column(options: ['default' => false])]
    private bool $isDefault = false;
}
