#pragma once

#include <unknwn.h>

class ListenerTypeClassFactory final : public IClassFactory {
 public:
  ListenerTypeClassFactory();
  ListenerTypeClassFactory(const ListenerTypeClassFactory&) = delete;
  ListenerTypeClassFactory& operator=(const ListenerTypeClassFactory&) = delete;
  ~ListenerTypeClassFactory();

  STDMETHODIMP QueryInterface(REFIID iid, void** object) override;
  STDMETHODIMP_(ULONG) AddRef() override;
  STDMETHODIMP_(ULONG) Release() override;
  STDMETHODIMP CreateInstance(IUnknown* outer, REFIID iid, void** object) override;
  STDMETHODIMP LockServer(BOOL lock) override;

 private:
  LONG ref_count_ = 1;
};
