<?php

declare(strict_types=1);

namespace App\Entity;

use Doctrine\ORM\Mapping as ORM;
use Symfony\Component\Validator\Constraints as Assert;

/**
 * Attendu du scanner Doctrine :
 *   - table `bank_account`
 *   - accountNumber → colonne `iban` (name: explicite) → preset iban (Assert\Iban)
 *   - swift         → colonne `bic`                     → preset bic  (Assert\Bic)
 *   - holderName    → colonne `holder_name`             → full_name (nom)
 *   - isDefault     → keep (bool)
 *   - relation ManyToOne user → arête FK bank_account.user_id → user.id
 *
 * Piège : le nom de propriété `accountNumber` ne dit pas "iban", c'est
 * l'attribut de validation qui doit gagner.
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
